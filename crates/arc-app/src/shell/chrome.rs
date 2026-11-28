//! The window chrome around the dock: the View menu (which panels are shown, reopening a closed
//! panel, resetting the layout), the toolbar strip, the status bar, and the keyboard routing that
//! sends typing outside a text field to the command line. The decisions are pure functions so
//! tests run them without a window; the drawing is thin.

use std::sync::Arc;

use arc_base::error::Value;
use arc_ui::egui::{self, Ui};
use arc_ui::{
    Axis, Colors, Dock, DockLayout, Icons, Intent, LineAction, Localize, Message, Side, Target,
    resolve,
};

use super::Action;

use crate::application::{Application, TabId};

/// A shell-side action of a toolbar item or of the View menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAction {
    /// Frames the camera on everything drawn.
    ZoomExtents,
    /// Shows a panel that is not in the layout.
    ShowPanel(String),
    /// Hides a panel.
    HidePanel(String),
    /// Puts the product's default layout back.
    ResetLayout,
    /// Makes a document tab the active one.
    ActivateTab(TabId),
    /// Closes a document tab, asking about unsaved changes.
    CloseTab(TabId),
}

/// The options of a choice list of the toolbar and the entry shown as current.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChoiceState {
    /// The entries, in the order shown.
    pub options: Vec<String>,
    /// The current entry.
    pub current: Option<String>,
}

/// Reads the state of a toolbar choice list for the active tab.
pub type ChoiceProbe = Arc<dyn Fn(&Application, TabId) -> ChoiceState + Send + Sync>;
/// Turns a picked entry of a choice list into the intent that makes it current.
pub type ChoicePick = Arc<dyn Fn(&str) -> Intent + Send + Sync>;
/// Reads whether a status toggle is on for the active tab; `None` when it does not apply.
pub type TogglePerceive = Arc<dyn Fn(&Application, TabId) -> Option<bool> + Send + Sync>;
/// Turns the wanted state of a status toggle into the intent that sets it, from the state of the
/// application (a toggle may be one bit of a larger setting).
pub type ToggleSet = Arc<dyn Fn(&Application, TabId, bool) -> Option<Intent> + Send + Sync>;

/// An item of the toolbar strip.
#[derive(Clone)]
pub enum ToolItem {
    /// A button that submits `line` to the command line, as if the user typed it and pressed
    /// Enter. `label` is a message identifier; `hint` is another, shown as the tooltip.
    Button {
        /// The label's message identifier.
        label: String,
        /// The tooltip's message identifier.
        hint: Option<String>,
        /// The text submitted.
        line: String,
    },
    /// A button with an icon of the product's or the starter icon set; it submits `line` like
    /// [`ToolItem::Button`]. An unknown icon name, or an icon set that failed to build, shows
    /// the label as text.
    IconButton {
        /// The label's message identifier (also the accessible name).
        label: String,
        /// The tooltip's message identifier.
        hint: Option<String>,
        /// The text submitted.
        line: String,
        /// The icon name.
        icon: String,
        /// How the button is laid out.
        size: ButtonSize,
    },
    /// A list of choices with one current entry, such as the layer list.
    Choice {
        /// The tooltip's message identifier.
        hint: Option<String>,
        /// Reads the options and the current entry.
        probe: ChoiceProbe,
        /// The intent of picking an entry.
        pick: ChoicePick,
    },
    /// A button that frames the camera on the drawing.
    ZoomExtents {
        /// The label's message identifier.
        label: String,
    },
    /// [`ToolItem::ZoomExtents`] drawn like an [`ToolItem::IconButton`]; an unknown icon shows the
    /// label as text.
    ZoomExtentsIcon {
        /// The label's message identifier (also the accessible name).
        label: String,
        /// The tooltip's message identifier.
        hint: Option<String>,
        /// The icon name.
        icon: String,
        /// How the button is laid out.
        size: ButtonSize,
    },
    /// A gap between groups.
    Separator,
}

impl std::fmt::Debug for ToolItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Button { label, line, .. } => write!(f, "Button({label}, {line})"),
            Self::IconButton {
                label, line, icon, ..
            } => {
                write!(f, "IconButton({label}, {line}, {icon})")
            }
            Self::Choice { .. } => f.write_str("Choice"),
            Self::ZoomExtents { label } => write!(f, "ZoomExtents({label})"),
            Self::ZoomExtentsIcon { label, icon, .. } => {
                write!(f, "ZoomExtentsIcon({label}, {icon})")
            }
            Self::Separator => f.write_str("Separator"),
        }
    }
}

/// How an icon button is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    /// A 32 point icon above a centered label (two lines at most), 64 points wide.
    Large,
    /// A 16 point icon left of the label; consecutive small buttons stack three to a column.
    Small,
}

/// A labeled set of items of a command tab.
#[derive(Debug, Clone)]
pub struct CommandGroup {
    /// The label's message identifier, shown below the items.
    pub label: String,
    /// The items, left to right.
    pub items: Vec<ToolItem>,
}

/// Decides whether a contextual command tab is shown for a document tab.
#[derive(Clone)]
pub struct ContextProbe(pub Arc<ContextFn>);

/// The function of a [`ContextProbe`].
pub type ContextFn = dyn Fn(&Application, TabId) -> bool + Send + Sync;

impl std::fmt::Debug for ContextProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContextProbe")
    }
}

/// Reads status texts of the active tab, drawn right-aligned in the status bar.
#[derive(Clone)]
pub struct StatusProbe(pub Arc<StatusFn>);

/// The function of a [`StatusProbe`].
pub type StatusFn = dyn Fn(&Application, TabId, &dyn Localize) -> Vec<String> + Send + Sync;

impl std::fmt::Debug for StatusProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StatusProbe")
    }
}

/// One command tab: groups of items. With a `context`, the tab is shown only while the probe
/// is true for the active document tab.
#[derive(Debug, Clone)]
pub struct CommandTab {
    /// The label's message identifier.
    pub label: String,
    /// The groups, left to right.
    pub groups: Vec<CommandGroup>,
    /// Shows the tab only while true; `None` shows it always.
    pub context: Option<ContextProbe>,
}

/// Which command tabs are shown, given for each tab whether it is (always true without a probe).
#[must_use]
pub fn visible_tabs(shown: &[bool]) -> Vec<usize> {
    shown
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.then_some(i))
        .collect()
}

/// What the user chose among the command tabs, and what the frames before showed. Kept per
/// shell window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandTabsState {
    /// The last chosen tab that is not contextual.
    chosen: usize,
    /// The contextual tab that is current, while it is shown.
    contextual: Option<usize>,
    /// The tabs shown in the previous frame.
    previous: Vec<usize>,
    /// The panel is folded away; only the tab row shows.
    pub collapsed: bool,
}

impl CommandTabsState {
    /// Takes the tabs shown this frame (`visible`; `is_contextual` tells the contextual ones): a
    /// contextual tab that just appeared becomes current, and one that went away hands the
    /// current tab back to the tab chosen before.
    pub fn update(&mut self, visible: &[usize], is_contextual: &dyn Fn(usize) -> bool) {
        if let Some(new) = visible
            .iter()
            .rev()
            .find(|i| is_contextual(**i) && !self.previous.contains(i))
        {
            self.contextual = Some(*new);
        }
        if self.contextual.is_some_and(|c| !visible.contains(&c)) {
            self.contextual = None;
        }
        self.previous = visible.to_vec();
    }

    /// The current tab among the shown ones: the contextual one, else the chosen one, else the
    /// first shown.
    #[must_use]
    pub fn current(&self, visible: &[usize]) -> Option<usize> {
        self.contextual
            .filter(|c| visible.contains(c))
            .or_else(|| visible.contains(&self.chosen).then_some(self.chosen))
            .or_else(|| visible.first().copied())
    }

    /// The user picked tab `index`; a pick expands a collapsed panel.
    pub fn select(&mut self, index: usize, contextual: bool) {
        if contextual {
            self.contextual = Some(index);
        } else {
            self.chosen = index;
            self.contextual = None;
        }
        self.collapsed = false;
    }
}

/// Reads what the viewport of a tab draws besides the scene (grid, axes) from the document.
#[derive(Clone)]
pub struct DisplayProbe(pub Arc<DisplayFn>);

/// The function of a [`DisplayProbe`].
pub type DisplayFn = dyn Fn(&Application, TabId) -> arc_ui::ViewportDisplay + Send + Sync;

impl std::fmt::Debug for DisplayProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DisplayProbe")
    }
}

/// What each viewport draws over the scene in model coordinates (labels, markers), read from the
/// document for the active tab. The probe gets the active language tag, so a text it returns is
/// in the language and the number format the user reads. Overlays follow the camera; the shell
/// keeps labels that would cover each other apart (see [`arc_ui::ProductOverlay::Label`]).
#[derive(Clone)]
pub struct OverlayProbe(pub Arc<OverlayFn>);

/// The function of an [`OverlayProbe`].
pub type OverlayFn = dyn Fn(&Application, TabId, &str) -> Vec<arc_ui::ProductOverlay> + Send + Sync;

impl std::fmt::Debug for OverlayProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OverlayProbe")
    }
}

/// The plane the active tab's point input lands on, read from the document and the tab's
/// selection: a click that answers
/// a point prompt, and the model point of a pointer event the product's controller receives, meet
/// this plane (a storey's level plane, for example) instead of the plane through the camera
/// target. `None` from the probe keeps the camera plane.
#[derive(Clone)]
pub struct WorkPlaneProbe(pub Arc<WorkPlaneFn>);

/// The function of a [`WorkPlaneProbe`].
pub type WorkPlaneFn =
    dyn Fn(&Application, TabId, &[arc_base::ObjectId]) -> Option<arc_ui::WorkPlane> + Send + Sync;

impl std::fmt::Debug for WorkPlaneProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkPlaneProbe")
    }
}

/// The view a new tab's viewport starts in, framed on the drawing once its scene is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InitialView {
    /// The direction from the drawing toward the camera, in model coordinates with z up. A
    /// direction along the z axis looks straight down with y up on the screen.
    pub toward_camera: [f64; 3],
}

/// A toggle button of the status bar: a precision mode the product has a setting for.
#[derive(Clone)]
pub struct StatusToggle {
    /// The label's message identifier.
    pub label: String,
    /// The tooltip's message identifier.
    pub hint: Option<String>,
    /// Reads the state.
    pub state: TogglePerceive,
    /// The intent that sets the state.
    pub set: ToggleSet,
    /// A function key that flips the toggle, when it has one.
    pub key: Option<egui::Key>,
}

impl std::fmt::Debug for StatusToggle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StatusToggle({})", self.label)
    }
}

/// True when the command line should take keyboard focus because the frame's `events` carry
/// typing and no text field has it. Text goes to the command line unless a text field is
/// focused. Enter and Escape go there only when no other widget holds focus (a focused button
/// keeps its Enter); the viewport does not count as such a widget. Keys with Ctrl, Alt or the
/// command key are shortcuts and stay where they are.
#[must_use]
pub fn wants_command_line(
    events: &[egui::Event],
    text_field_focused: bool,
    other_widget_focused: bool,
) -> bool {
    if text_field_focused {
        return false;
    }
    events.iter().any(|event| match event {
        egui::Event::Text(text) => !text.is_empty() && !text.chars().any(char::is_control),
        egui::Event::Key {
            key: egui::Key::Enter | egui::Key::Escape,
            pressed: true,
            modifiers,
            ..
        } => !modifiers.any() && !other_widget_focused,
        _ => false,
    })
}

/// Where a closed panel comes back: with a group-mate that is shown, else beside the shown
/// part of the product's default layout it was next to, else on the right. Returns the target and
/// the share the panel's side of the new split gets, taken from the default layout.
#[must_use]
pub fn reopen_target(
    default: &DockLayout,
    present: &dyn Fn(&str) -> bool,
    panel: &str,
) -> (Target, Option<f32>) {
    match locate(default.root(), panel, present, true) {
        Some((Some(found), share)) => (found, share),
        _ => (Target::Root { side: Side::Right }, None),
    }
}

/// What [`locate`] found for a panel in a subtree: the target and the split share, or nothing
/// resolved.
type Located = Option<(Option<Target>, Option<f32>)>;

fn first_present(dock: &Dock, present: &dyn Fn(&str) -> bool) -> Option<String> {
    match dock {
        Dock::Tabs { tabs, .. } => tabs.iter().find(|t| present(t)).cloned(),
        Dock::Split { first, second, .. } => {
            first_present(first, present).or_else(|| first_present(second, present))
        }
    }
}

fn locate(dock: &Dock, panel: &str, present: &dyn Fn(&str) -> bool, at_root: bool) -> Located {
    match dock {
        Dock::Tabs { tabs, .. } => {
            if !tabs.iter().any(|t| t == panel) {
                return None;
            }
            let mate = tabs.iter().find(|t| *t != panel && present(t));
            Some((
                mate.map(|anchor| Target::Tab {
                    anchor: anchor.clone(),
                    index: None,
                }),
                None,
            ))
        }
        Dock::Split {
            axis,
            fraction,
            first,
            second,
        } => {
            let (in_first, child, sibling) = if locate(first, panel, present, false).is_some() {
                (true, first, second)
            } else if locate(second, panel, present, false).is_some() {
                (false, second, first)
            } else {
                return None;
            };
            let inner = locate(child, panel, present, false)?;
            if inner.0.is_some() {
                return Some(inner);
            }
            let Some(anchor) = first_present(sibling, present) else {
                return Some((None, None));
            };
            let side = match (axis, in_first) {
                (Axis::Horizontal, true) => Side::Left,
                (Axis::Horizontal, false) => Side::Right,
                (Axis::Vertical, true) => Side::Top,
                (Axis::Vertical, false) => Side::Bottom,
            };
            let share = if in_first { *fraction } else { 1.0 - *fraction };
            // A split at the root divides the whole main area; deeper ones divide one group.
            let target = if at_root {
                Target::Root { side }
            } else {
                Target::Split { anchor, side }
            };
            Some((Some(target), Some(share)))
        }
    }
}

/// Shows `panel` again: [`reopen_target`] places it and the new split gets the default share.
///
/// # Errors
/// `ui.dock.panel_duplicate` when it is shown already; the dock errors of the result.
pub fn show_panel(
    layout: &mut DockLayout,
    default: &DockLayout,
    panel: &str,
) -> arc_base::Result<()> {
    let present = |p: &str| layout.contains(p);
    let (target, share) = reopen_target(default, &present, panel);
    layout.add(panel, &target)?;
    if let Some(share) = share {
        let mut path = Vec::new();
        if find_group(layout.root(), panel, &mut path) {
            // The new group is one child of the split that now holds it: the path of that split
            // is the group's path without its last step.
            let last = path.pop();
            if let Some(second) = last {
                layout.set_fraction(&path, if second { 1.0 - share } else { share });
            }
        }
    }
    Ok(())
}

/// Finds the group that holds exactly `panel`, recording the steps to it.
fn find_group(dock: &Dock, panel: &str, path: &mut Vec<bool>) -> bool {
    match dock {
        Dock::Tabs { tabs, .. } => tabs.len() == 1 && tabs.first().is_some_and(|t| t == panel),
        Dock::Split { first, second, .. } => {
            path.push(false);
            if find_group(first, panel, path) {
                return true;
            }
            path.pop();
            path.push(true);
            if find_group(second, panel, path) {
                return true;
            }
            path.pop();
            false
        }
    }
}

/// One row of the View menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    /// The panel identifier.
    pub id: String,
    /// The title.
    pub title: String,
    /// Whether the layout shows it.
    pub shown: bool,
}

/// The View menu's panel rows: every registered panel with its state.
#[must_use]
pub fn panel_rows(layout: &DockLayout, panels: &[(String, String)]) -> Vec<PanelRow> {
    panels
        .iter()
        .map(|(id, title)| PanelRow {
            id: id.clone(),
            title: title.clone(),
            shown: layout.contains(id),
        })
        .collect()
}

const MENU_VIEW: Message = Message {
    message_id: "app-menu-view",
};
const MENU_RESET: Message = Message {
    message_id: "app-menu-view-reset-layout",
};
const MENU_ZOOM: Message = Message {
    message_id: "app-menu-view-zoom-extents",
};
const TAB_CLOSE: Message = Message {
    message_id: "app-tab-close",
};
const STATUS_CURSOR: Message = Message {
    message_id: "app-status-cursor",
};
const STATUS_NO_CURSOR: Message = Message {
    message_id: "app-status-no-cursor",
};

/// Draws the View menu into the menu bar `ui`: a check mark per panel (clicking toggles it),
/// zoom to the extents and the layout reset. Returns what the user chose.
pub fn view_menu(ui: &mut Ui, rows: &[PanelRow], localize: &dyn Localize) -> Option<ShellAction> {
    let mut chosen = None;
    ui.menu_button(MENU_VIEW.text(localize), |ui| {
        for row in rows {
            let mut shown = row.shown;
            if ui.checkbox(&mut shown, row.title.as_str()).clicked() {
                chosen = Some(if shown {
                    ShellAction::ShowPanel(row.id.clone())
                } else {
                    ShellAction::HidePanel(row.id.clone())
                });
                ui.close();
            }
        }
        ui.separator();
        if ui.button(MENU_ZOOM.text(localize)).clicked() {
            chosen = Some(ShellAction::ZoomExtents);
            ui.close();
        }
        if ui.button(MENU_RESET.text(localize)).clicked() {
            chosen = Some(ShellAction::ResetLayout);
            ui.close();
        }
    });
    chosen
}

/// The intents of the toggles whose key was pressed in `events` (without modifiers), in order.
#[must_use]
pub fn toggle_key_intents(
    toggles: &[StatusToggle],
    views: &[ToggleView],
    events: &[egui::Event],
) -> Vec<Intent> {
    let mut out = Vec::new();
    for event in events {
        let egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } = event
        else {
            continue;
        };
        if modifiers.any() {
            continue;
        }
        for (toggle, view) in toggles.iter().zip(views) {
            if toggle.key == Some(*key)
                && let Some(flip) = &view.flip
            {
                out.push(flip.clone());
            }
        }
    }
    out
}

/// One document tab of the tab strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabView {
    /// The tab.
    pub id: TabId,
    /// The name shown: the file name, or the shell's name for a document never saved.
    pub title: String,
    /// The document has unsaved changes.
    pub modified: bool,
    /// The tab is the active one.
    pub active: bool,
}

/// What the chrome of a frame shows: the menu bar, the toolbar strip, the document tabs and the
/// status bar.
pub struct ChromeFrame<'a> {
    /// The document tabs, in order.
    pub tabs: &'a [TabView],
    /// The resolved menu bar.
    pub entries: &'a [arc_ui::Entry],
    /// The rows of the View menu.
    pub rows: &'a [PanelRow],
    /// The toolbar items.
    pub tools: &'a [ToolView],
    /// The status toggles.
    pub toggles: &'a [ToggleView],
    /// The cursor text of the status bar.
    pub cursor: &'a str,
    /// The status texts of the product, right-aligned in the status bar.
    pub status_text: &'a [String],
    /// The command tabs; they replace the flat toolbar strip when present.
    pub commands: Option<&'a CommandBarView>,
    /// The quick access items, drawn in the menu bar row.
    pub quick: &'a [ToolView],
    /// The icons and the colors the command surface draws with.
    pub look: Look<'a>,
    /// The message source.
    pub localize: &'a dyn Localize,
}

/// Draws the menu bar (with the View menu), the toolbar strip and the status bar around the
/// dock; what the user chose becomes actions.
pub fn show_frame(ui: &mut Ui, frame: &ChromeFrame<'_>, actions: &mut Vec<Action>) {
    egui::Panel::top("arc-app-menu").show(ui, |ui| {
        let (intent, (chosen, quick)) = arc_ui::menu::show_menu_bar_with(ui, frame.entries, |ui| {
            let chosen = view_menu(ui, frame.rows, frame.localize);
            let mut quick = None;
            for view in frame.quick {
                draw_view(ui, view, &frame.look, true, &mut quick);
            }
            (chosen, quick)
        });
        actions.extend(intent.map(Action::Start));
        actions.extend(chosen.map(Action::Shell));
        push_reply(quick, actions);
    });
    if let Some(bar) = frame.commands {
        let fill = frame.look.colors.surface_raised;
        egui::Panel::top("arc-app-commands")
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| {
                push_reply(show_commands(ui, bar, &frame.look), actions)
            });
    } else if !frame.tools.is_empty() {
        egui::Panel::top("arc-app-tools").show(ui, |ui| {
            push_reply(show_tools(ui, frame.tools, &frame.look), actions);
        });
    }
    if !frame.tabs.is_empty() {
        egui::Panel::top("arc-app-tabs").show(ui, |ui| {
            if let Some(action) = show_tabs(ui, frame.tabs, frame.localize) {
                actions.push(Action::Shell(action));
            }
        });
    }
    egui::Panel::bottom("arc-app-status").show(ui, |ui| {
        if let Some(intent) = show_status(ui, frame.cursor, frame.toggles, frame.status_text) {
            actions.push(Action::Line(LineAction::Invoke(intent)));
        }
    });
}

fn push_reply(reply: Option<ToolReply>, actions: &mut Vec<Action>) {
    match reply {
        Some(ToolReply::Line(line)) => actions.push(Action::Line(LineAction::Submit(line))),
        // Through the line: a product's controller runs it with the parameters given,
        // without prompting for them.
        Some(ToolReply::Intent(intent)) => actions.push(Action::Line(LineAction::Invoke(intent))),
        Some(ToolReply::Shell(action)) => actions.push(Action::Shell(action)),
        None => {}
    }
}

/// The modified mark of a document, shown after its name in the tab strip.
pub const MODIFIED_MARK: &str = "*";

/// The label of a tab: the name, then the modified mark.
#[must_use]
pub fn tab_label(tab: &TabView) -> String {
    if tab.modified {
        format!("{}{MODIFIED_MARK}", tab.title)
    } else {
        tab.title.clone()
    }
}

/// Draws one tab per open document: its name with the modified mark (a click activates it) and
/// a close button. Returns what the user chose.
pub fn show_tabs(ui: &mut Ui, tabs: &[TabView], localize: &dyn Localize) -> Option<ShellAction> {
    let mut chosen = None;
    let close_name = TAB_CLOSE.text(localize);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for tab in tabs {
            let (tabr, x) = arc_ui::flat_tab(
                ui,
                &tab_label(tab),
                &tab.title,
                tab.active,
                &close_name,
                true,
            );
            if x.clicked() {
                chosen = Some(ShellAction::CloseTab(tab.id));
            } else if tabr.clicked() && !tab.active {
                chosen = Some(ShellAction::ActivateTab(tab.id));
            }
        }
    });
    chosen
}

/// What the toolbar strip does after a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolReply {
    /// Submit this text to the command line.
    Line(String),
    /// Carry out this intent.
    Intent(Intent),
    /// A shell action.
    Shell(ShellAction),
}

/// A toolbar item with its state read for this frame.
pub enum ToolView {
    /// A button.
    Button {
        /// The label.
        label: String,
        /// The tooltip.
        hint: Option<String>,
        /// The text it submits.
        line: String,
    },
    /// An icon button.
    Icon {
        /// The label.
        label: String,
        /// The tooltip.
        hint: Option<String>,
        /// The text it submits.
        line: String,
        /// The icon name.
        icon: String,
        /// The layout.
        size: ButtonSize,
    },
    /// A choice list.
    Choice {
        /// The tooltip.
        hint: Option<String>,
        /// The options and the current one.
        state: ChoiceState,
        /// Picking an entry.
        pick: ChoicePick,
    },
    /// Zoom to the extents.
    Zoom {
        /// The label.
        label: String,
    },
    /// Zoom to the extents, drawn as an icon button.
    ZoomIcon {
        /// The label.
        label: String,
        /// The tooltip.
        hint: Option<String>,
        /// The icon name.
        icon: String,
        /// The layout.
        size: ButtonSize,
    },
    /// A gap.
    Separator,
}

/// Reads the toolbar items for the active tab and localizes their labels.
#[must_use]
pub fn tool_views(
    items: &[ToolItem],
    app: &Application,
    tab: Option<TabId>,
    localize: &dyn Localize,
) -> Vec<ToolView> {
    let text = |id: &str| resolve(localize, id, &[]);
    items
        .iter()
        .map(|item| match item {
            ToolItem::Button { label, hint, line } => ToolView::Button {
                label: text(label),
                hint: hint.as_deref().map(text),
                line: line.clone(),
            },
            ToolItem::IconButton {
                label,
                hint,
                line,
                icon,
                size,
            } => ToolView::Icon {
                label: text(label),
                hint: hint.as_deref().map(text),
                line: line.clone(),
                icon: icon.clone(),
                size: *size,
            },
            ToolItem::Choice { hint, probe, pick } => ToolView::Choice {
                hint: hint.as_deref().map(text),
                state: tab.map(|t| probe(app, t)).unwrap_or_default(),
                pick: Arc::clone(pick),
            },
            ToolItem::ZoomExtents { label } => ToolView::Zoom { label: text(label) },
            ToolItem::ZoomExtentsIcon {
                label,
                hint,
                icon,
                size,
            } => ToolView::ZoomIcon {
                label: text(label),
                hint: hint.as_deref().map(text),
                icon: icon.clone(),
                size: *size,
            },
            ToolItem::Separator => ToolView::Separator,
        })
        .collect()
}

/// What the command surface draws with: the uploaded icons (`None` shows labels as text) and
/// the theme's colors.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    /// The icon atlas, when it built.
    pub icons: Option<&'a Icons>,
    /// The theme colors.
    pub colors: &'a Colors,
}

const LARGE_W: f32 = 64.0;
const LARGE_ICON: f32 = 32.0;
const SMALL_ICON: f32 = 16.0;
const QUICK_SIZE: f32 = 24.0;
const ROW_H: f32 = 72.0;
const SMALL_H: f32 = 22.0;
const GROUP_LABEL_H: f32 = 14.0;
const PAD: f32 = 6.0;
/// Horizontal padding of a command tab label, and the height of the tab row.
const TAB_PAD: f32 = 12.0;
const TAB_H: f32 = 26.0;
const ROUNDING: u8 = 4;
/// The most small buttons in one column.
const STACK: usize = 3;

/// How an icon button is drawn: [`ButtonSize`] or icon-only for the quick access row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Large,
    Small,
    Quick,
}

/// Paints a large button's content: the icon above the label, or the label alone without an icon.
fn paint_large(
    ui: &Ui,
    rect: egui::Rect,
    image: Option<egui::Image<'_>>,
    label: &str,
    font: egui::FontId,
    colors: &Colors,
) {
    let mut job =
        egui::text::LayoutJob::simple(label.to_owned(), font, colors.text, rect.width() - 8.0);
    job.halign = egui::Align::Center;
    let top = if let Some(image) = image {
        let at = egui::pos2(rect.center().x, rect.top() + PAD + LARGE_ICON / 2.0);
        image.paint_at(
            ui,
            egui::Rect::from_center_size(at, egui::Vec2::splat(LARGE_ICON)),
        );
        job.wrap.max_rows = 2;
        Some(rect.top() + PAD + LARGE_ICON + 2.0)
    } else {
        job.wrap.max_rows = 3;
        None
    };
    let galley = ui.painter().layout_job(job);
    let top = top.unwrap_or(rect.center().y - galley.size().y / 2.0);
    ui.painter()
        .galley(egui::pos2(rect.center().x, top), galley, colors.text);
}

/// Draws one icon button: hover and pressed fills, the icon (or the label as text when there is
/// no such icon), the label as accessible name and the tooltip.
fn icon_button(
    ui: &mut Ui,
    look: &Look<'_>,
    label: &str,
    hint: Option<&str>,
    icon: &str,
    form: Form,
) -> egui::Response {
    let colors = look.colors;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let image = |edge: f32| {
        look.icons
            .and_then(|icons| icons.image(icon, egui::vec2(edge, edge), colors.text))
    };
    let icon_edge = if form == Form::Large {
        LARGE_ICON
    } else {
        SMALL_ICON
    };
    let image = image(icon_edge);
    let beside_text = |ui: &Ui| {
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font.clone(), colors.text);
        let lead = if image.is_some() {
            SMALL_ICON + 4.0
        } else {
            0.0
        };
        (
            galley.clone(),
            egui::vec2(2.0 * PAD + lead + galley.size().x, SMALL_H),
        )
    };
    let (size, galley) = match (form, &image) {
        // Wide enough for the label's longest word, so wrapping never splits a word.
        (Form::Large, _) => {
            let widest = label
                .split_whitespace()
                .map(|word| {
                    ui.painter()
                        .layout_no_wrap(word.to_owned(), font.clone(), colors.text)
                        .size()
                        .x
                })
                .fold(0.0_f32, f32::max);
            (egui::vec2(LARGE_W.max(widest + 8.0), ROW_H), None)
        }
        (Form::Quick, Some(_)) => (egui::vec2(QUICK_SIZE, QUICK_SIZE), None),
        _ => {
            let (galley, size) = beside_text(ui);
            (size, Some(galley))
        }
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.is_pointer_button_down_on() {
            Some(colors.selection)
        } else if response.hovered() {
            Some(colors.control_hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            painter.rect_filled(rect, egui::CornerRadius::same(ROUNDING), fill);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(ROUNDING),
                egui::Stroke::new(1.5, colors.focus),
                egui::StrokeKind::Inside,
            );
        }
        match (form, image, galley) {
            (Form::Large, image, _) => paint_large(ui, rect, image, label, font, colors),
            (Form::Quick, Some(image), _) => {
                image.paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(SMALL_ICON)),
                );
            }
            (_, image, Some(galley)) => {
                let mut x = rect.left() + PAD;
                if let Some(image) = image {
                    let at = egui::pos2(x + SMALL_ICON / 2.0, rect.center().y);
                    image.paint_at(
                        ui,
                        egui::Rect::from_center_size(at, egui::Vec2::splat(SMALL_ICON)),
                    );
                    x += SMALL_ICON + 4.0;
                }
                let at = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
                ui.painter().galley(at, galley, colors.text);
            }
            _ => {}
        }
    }
    let tip = match (form, hint) {
        (Form::Quick, Some(hint)) => Some(format!("{label}\n{hint}")),
        (Form::Quick, None) => Some(label.to_owned()),
        (_, hint) => hint.map(str::to_owned),
    };
    match tip {
        Some(tip) => response.on_hover_text(tip),
        None => response,
    }
}

/// Draws one toolbar item; a click becomes the reply. `quick` draws icon buttons icon-only.
fn draw_view(
    ui: &mut Ui,
    view: &ToolView,
    look: &Look<'_>,
    quick: bool,
    reply: &mut Option<ToolReply>,
) {
    match view {
        ToolView::Button { label, hint, line } => {
            let mut response = ui.button(label.as_str());
            if let Some(hint) = hint {
                response = response.on_hover_text(hint.as_str());
            }
            if response.clicked() {
                *reply = Some(ToolReply::Line(line.clone()));
            }
        }
        ToolView::Icon {
            label,
            hint,
            line,
            icon,
            size,
        } => {
            let form = match (quick, size) {
                (true, _) => Form::Quick,
                (false, ButtonSize::Large) => Form::Large,
                (false, ButtonSize::Small) => Form::Small,
            };
            if icon_button(ui, look, label, hint.as_deref(), icon, form).clicked() {
                *reply = Some(ToolReply::Line(line.clone()));
            }
        }
        ToolView::Zoom { label } => {
            if ui.button(label.as_str()).clicked() {
                *reply = Some(ToolReply::Shell(ShellAction::ZoomExtents));
            }
        }
        ToolView::ZoomIcon {
            label,
            hint,
            icon,
            size,
        } => {
            let form = match (quick, size) {
                (true, _) => Form::Quick,
                (false, ButtonSize::Large) => Form::Large,
                (false, ButtonSize::Small) => Form::Small,
            };
            if icon_button(ui, look, label, hint.as_deref(), icon, form).clicked() {
                *reply = Some(ToolReply::Shell(ShellAction::ZoomExtents));
            }
        }
        ToolView::Choice { hint, state, pick } => {
            let current = state.current.clone().unwrap_or_default();
            let combo = egui::ComboBox::from_id_salt("arc-toolbar-choice")
                .selected_text(current.as_str())
                .show_ui(ui, |ui| {
                    for option in &state.options {
                        let selected = state.current.as_deref() == Some(option.as_str());
                        if ui.selectable_label(selected, option.as_str()).clicked() && !selected {
                            *reply = Some(ToolReply::Intent(pick(option)));
                        }
                    }
                });
            if let Some(hint) = hint {
                combo.response.on_hover_text(hint.as_str());
            }
        }
        ToolView::Separator => {
            ui.separator();
        }
    }
}

/// Draws the flat toolbar strip and returns what the user clicked.
pub fn show_tools(ui: &mut Ui, views: &[ToolView], look: &Look<'_>) -> Option<ToolReply> {
    let mut reply = None;
    ui.horizontal_wrapped(|ui| {
        for view in views {
            draw_view(ui, view, look, false, &mut reply);
        }
    });
    reply
}

/// One tab of the command tab row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabHead {
    /// The tab's index in the configuration.
    pub index: usize,
    /// The label.
    pub label: String,
    /// The tab is contextual.
    pub contextual: bool,
}

/// A group of the current command tab with its items read.
pub struct GroupView {
    /// The label.
    pub label: String,
    /// The items.
    pub items: Vec<ToolView>,
}

/// The command tabs of a frame: the shown tabs and the groups of the current one.
pub struct CommandBarView {
    /// Where the per-window state is kept.
    pub state_id: egui::Id,
    /// The shown tabs, in order.
    pub heads: Vec<TabHead>,
    /// The configuration index of the current tab.
    pub current: usize,
    /// The panel is folded away.
    pub collapsed: bool,
    /// The groups of the current tab.
    pub groups: Vec<GroupView>,
}

/// Whether the flat toolbar strip is drawn: command tabs replace it in the main window, also while
/// every tab is contextual and none is shown (the panel is then empty, not the toolbar).
#[must_use]
pub fn flat_toolbar_shown(tabs_configured: bool, main: bool) -> bool {
    !(tabs_configured && main)
}

/// Reads the command tabs for the active tab: which are shown (probes), which is current (the
/// per-window state kept in `ctx` under `state_id`) and the items of the current one. `None`
/// when no tab is shown.
#[must_use]
pub fn command_bar(
    ctx: &egui::Context,
    state_id: egui::Id,
    tabs: &[CommandTab],
    app: &Application,
    active: Option<TabId>,
    localize: &dyn Localize,
) -> Option<CommandBarView> {
    let shown: Vec<bool> = tabs
        .iter()
        .map(|t| match &t.context {
            None => true,
            Some(probe) => active.is_some_and(|tab| (probe.0)(app, tab)),
        })
        .collect();
    let visible = visible_tabs(&shown);
    let mut state: CommandTabsState = ctx.data_mut(|d| d.get_temp(state_id)).unwrap_or_default();
    state.update(&visible, &|i| {
        tabs.get(i).is_some_and(|t| t.context.is_some())
    });
    let current = state.current(&visible)?;
    let collapsed = state.collapsed;
    ctx.data_mut(|d| d.insert_temp(state_id, state));
    let heads = visible
        .iter()
        .filter_map(|&i| {
            let tab = tabs.get(i)?;
            Some(TabHead {
                index: i,
                label: resolve(localize, &tab.label, &[]),
                contextual: tab.context.is_some(),
            })
        })
        .collect();
    let groups = tabs.get(current).map_or_else(Vec::new, |tab| {
        tab.groups
            .iter()
            .map(|g| GroupView {
                label: resolve(localize, &g.label, &[]),
                items: tool_views(&g.items, app, active, localize),
            })
            .collect()
    });
    Some(CommandBarView {
        state_id,
        heads,
        current,
        collapsed,
        groups,
    })
}

fn edit_state(ctx: &egui::Context, id: egui::Id, edit: impl FnOnce(&mut CommandTabsState)) {
    let mut state: CommandTabsState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    edit(&mut state);
    ctx.data_mut(|d| d.insert_temp(id, state));
    ctx.request_repaint();
}

/// Draws the command tab row and, unless collapsed, the panel with the groups of the current
/// tab. Returns what the user clicked.
pub fn show_commands(ui: &mut Ui, bar: &CommandBarView, look: &Look<'_>) -> Option<ToolReply> {
    let colors = look.colors;
    let mut reply = None;
    // Ctrl+F1 folds and unfolds the panel from anywhere.
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F1)) {
        edit_state(ui.ctx(), bar.state_id, |s| s.collapsed = !s.collapsed);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for head in &bar.heads {
            let current = head.index == bar.current;
            let color = if head.contextual {
                colors.accent
            } else if current {
                colors.text
            } else {
                colors.text_muted
            };
            let galley = ui.painter().layout_no_wrap(
                head.label.clone(),
                egui::TextStyle::Button.resolve(ui.style()),
                color,
            );
            let size = egui::vec2(galley.size().x + 2.0 * TAB_PAD, TAB_H);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Tab, true, current, head.label.as_str())
            });
            if response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(ROUNDING),
                    colors.control_hover,
                );
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(ROUNDING),
                    egui::Stroke::new(1.5, colors.focus),
                    egui::StrokeKind::Inside,
                );
            }
            let at = egui::pos2(
                rect.left() + TAB_PAD,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(at, galley, color);
            if current {
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 1.0,
                    egui::Stroke::new(2.0, colors.accent),
                );
            }
            // A double click, or Enter or Space on the focused current tab, folds the panel.
            let key = current
                && response.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space));
            if (response.double_clicked() && current) || key {
                edit_state(ui.ctx(), bar.state_id, |s| s.collapsed = !s.collapsed);
            } else if response.clicked() && !current {
                edit_state(ui.ctx(), bar.state_id, |s| {
                    s.select(head.index, head.contextual)
                });
            }
        }
    });
    if bar.collapsed {
        return reply;
    }
    ui.add_space(2.0);
    egui::ScrollArea::horizontal()
        .id_salt("arc-command-panel")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (n, group) in bar.groups.iter().enumerate() {
                    if n > 0 {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(1.0, ROW_H + GROUP_LABEL_H),
                            egui::Sense::hover(),
                        );
                        ui.painter().vline(
                            rect.center().x,
                            rect.y_range(),
                            egui::Stroke::new(1.0, colors.border),
                        );
                    }
                    show_group(ui, group, look, &mut reply);
                }
            });
        });
    reply
}

/// One group: the items (consecutive small buttons in columns of three), then the label.
fn show_group(ui: &mut Ui, group: &GroupView, look: &Look<'_>, reply: &mut Option<ToolReply>) {
    let is_small = |v: &ToolView| {
        matches!(
            v,
            ToolView::Icon {
                size: ButtonSize::Small,
                ..
            } | ToolView::ZoomIcon {
                size: ButtonSize::Small,
                ..
            }
        )
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let row = ui.horizontal(|ui| {
            ui.set_min_height(ROW_H);
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut items = group.items.iter().peekable();
            while let Some(item) = items.next() {
                if is_small(item) {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        draw_view(ui, item, look, false, reply);
                        for _ in 1..STACK {
                            match items.next_if(|v| is_small(v)) {
                                Some(next) => draw_view(ui, next, look, false, reply),
                                None => break,
                            }
                        }
                    });
                } else {
                    draw_view(ui, item, look, false, reply);
                }
            }
        });
        ui.allocate_ui_with_layout(
            egui::vec2(row.response.rect.width(), GROUP_LABEL_H),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(group.label.as_str())
                            .small()
                            .color(look.colors.text_muted),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            },
        );
    });
}

/// A status toggle with its state read for this frame.
pub struct ToggleView {
    /// The label.
    pub label: String,
    /// The tooltip.
    pub hint: Option<String>,
    /// The state; `None` shows the toggle disabled.
    pub on: Option<bool>,
    /// The intent that flips the state.
    pub flip: Option<Intent>,
}

/// Reads the status toggles for the active tab.
#[must_use]
pub fn toggle_views(
    toggles: &[StatusToggle],
    app: &Application,
    tab: Option<TabId>,
    localize: &dyn Localize,
) -> Vec<ToggleView> {
    toggles
        .iter()
        .map(|t| {
            let on = tab.and_then(|tab| (t.state)(app, tab));
            ToggleView {
                label: resolve(localize, &t.label, &[]),
                hint: t.hint.as_deref().map(|h| resolve(localize, h, &[])),
                on,
                flip: tab.zip(on).and_then(|(tab, on)| (t.set)(app, tab, !on)),
            }
        })
        .collect()
}

/// The cursor text of the status bar: the drawing coordinates, or a placeholder.
#[must_use]
pub fn cursor_text(cursor: Option<[f64; 2]>, digits: usize, localize: &dyn Localize) -> String {
    match cursor {
        Some([x, y]) => resolve(
            localize,
            STATUS_CURSOR.message_id,
            &[
                ("x", Value::Text(format!("{x:.digits$}"))),
                ("y", Value::Text(format!("{y:.digits$}"))),
            ],
        ),
        None => STATUS_NO_CURSOR.text(localize),
    }
}

/// Draws the status bar content: the cursor coordinates on the left, the toggles after them and
/// the product's status texts at the right edge.
/// Returns the intent of a toggled mode.
pub fn show_status(
    ui: &mut Ui,
    cursor: &str,
    toggles: &[ToggleView],
    texts: &[String],
) -> Option<Intent> {
    let mut intent = None;
    ui.horizontal(|ui| {
        // A fixed width, so the toggles do not move with the length of the coordinates.
        let width = ui.text_style_height(&egui::TextStyle::Monospace) * 0.62 * 28.0;
        let used = ui
            .label(egui::RichText::new(cursor).monospace())
            .rect
            .width();
        ui.add_space((width - used).max(0.0));
        ui.separator();
        for toggle in toggles {
            let on = toggle.on.unwrap_or(false);
            let mut response = ui.add_enabled(
                toggle.on.is_some(),
                egui::Button::selectable(on, toggle.label.as_str()),
            );
            if let Some(hint) = &toggle.hint {
                response = response.on_hover_text(hint.as_str());
            }
            if response.clicked() {
                intent = toggle.flip.clone();
            }
        }
        if !texts.is_empty() {
            // Right to left: the last text is added first, so they read in order.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (n, text) in texts.iter().enumerate().rev() {
                    ui.label(text.as_str());
                    if n > 0 {
                        ui.separator();
                    }
                }
            });
        }
    });
    intent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> egui::Event {
        egui::Event::Text(t.to_owned())
    }

    fn key(k: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn typing_outside_a_text_field_goes_to_the_command_line() {
        assert!(wants_command_line(&[text("L")], false, false));
        assert!(wants_command_line(&[text("L")], false, true));
        let enter = key(egui::Key::Enter, egui::Modifiers::NONE);
        assert!(wants_command_line(
            std::slice::from_ref(&enter),
            false,
            false
        ));
        assert!(wants_command_line(
            &[key(egui::Key::Escape, egui::Modifiers::NONE)],
            false,
            false
        ));
        // A focused button keeps its Enter.
        assert!(!wants_command_line(&[enter], false, true));
        // A text field that has focus keeps its keys; shortcuts and control text stay put.
        assert!(!wants_command_line(&[text("L")], true, false));
        assert!(!wants_command_line(
            &[key(egui::Key::Enter, egui::Modifiers::CTRL)],
            false,
            false
        ));
        assert!(!wants_command_line(&[text("\u{7f}")], false, false));
        assert!(!wants_command_line(
            &[egui::Event::PointerGone],
            false,
            false
        ));
    }

    fn layout() -> DockLayout {
        let tabs = |p: &str| Dock::Tabs {
            tabs: vec![p.to_owned()],
            active: 0,
        };
        let right = Dock::Split {
            axis: Axis::Vertical,
            fraction: 0.4,
            first: Box::new(tabs("outliner")),
            second: Box::new(tabs("properties")),
        };
        let top = Dock::Split {
            axis: Axis::Horizontal,
            fraction: 0.7,
            first: Box::new(tabs("viewport")),
            second: Box::new(right),
        };
        let root = Dock::Split {
            axis: Axis::Vertical,
            fraction: 0.8,
            first: Box::new(top),
            second: Box::new(tabs("line")),
        };
        DockLayout::new(root, Vec::new()).unwrap()
    }

    #[test]
    fn a_closed_panel_can_be_shown_again_in_its_place() {
        let default = layout();
        for panel in ["viewport", "outliner", "properties", "line"] {
            let mut current = default.clone();
            current.close(panel).unwrap();
            assert!(!current.contains(panel));
            show_panel(&mut current, &default, panel).unwrap();
            assert!(current.contains(panel), "{panel}");
        }
        // The command line comes back below the whole area at its share, not at half.
        let mut current = default.clone();
        current.close("line").unwrap();
        show_panel(&mut current, &default, "line").unwrap();
        assert_eq!(current, default);
        // Every panel closed: the first one shown fills the main area.
        let mut empty = default.clone();
        for panel in default.panels() {
            empty.close(&panel).unwrap();
        }
        assert!(empty.panels().is_empty());
        show_panel(&mut empty, &default, "viewport").unwrap();
        show_panel(&mut empty, &default, "line").unwrap();
        assert_eq!(empty.panels().len(), 2);
    }

    #[test]
    fn an_unknown_panel_opens_on_the_right() {
        let default = layout();
        let mut current = default.clone();
        show_panel(&mut current, &default, "log").unwrap();
        assert!(current.contains("log"));
    }

    #[test]
    fn command_tabs_follow_the_probes() {
        assert_eq!(visible_tabs(&[true, false, true]), [0, 2]);
        // Tab 2 is contextual.
        let contextual = |i: usize| i == 2;
        let mut state = CommandTabsState::default();
        state.update(&[0, 1], &contextual);
        assert_eq!(state.current(&[0, 1]), Some(0));
        state.select(1, false);
        assert_eq!(state.current(&[0, 1]), Some(1));
        // The contextual tab appears: it becomes current by itself.
        state.update(&[0, 1, 2], &contextual);
        assert_eq!(state.current(&[0, 1, 2]), Some(2));
        // The user may go to another tab while it stays shown; it does not take over again.
        state.select(0, false);
        state.update(&[0, 1, 2], &contextual);
        assert_eq!(state.current(&[0, 1, 2]), Some(0));
        // It goes away while current: the tab chosen before comes back.
        state.select(2, true);
        state.update(&[0, 1], &contextual);
        assert_eq!(state.current(&[0, 1]), Some(0));
        // A chosen tab that is no longer shown falls back to the first one.
        assert_eq!(state.current(&[1]), Some(1));
        assert_eq!(state.current(&[]), None);
    }

    #[test]
    fn command_tabs_tell_assistive_technology_which_is_current() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let look = Look {
            icons: None,
            colors: &colors,
        };
        let head = |index: usize, label: &str| TabHead {
            index,
            label: label.into(),
            contextual: false,
        };
        let bar = CommandBarView {
            state_id: egui::Id::NULL.with("tabs"),
            heads: vec![head(0, "Home"), head(1, "Draw")],
            current: 1,
            collapsed: true,
            groups: Vec::new(),
        };
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_commands(ui, &bar, &look);
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.unwrap();
        let toggled = |label: &str| {
            update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == egui::accesskit::Role::Tab && n.label() == Some(label))
                .and_then(|(_, n)| n.toggled())
        };
        assert_eq!(toggled("Home"), Some(egui::accesskit::Toggled::False));
        assert_eq!(toggled("Draw"), Some(egui::accesskit::Toggled::True));
    }

    #[test]
    fn the_flat_toolbar_does_not_replace_hidden_command_tabs() {
        // Every tab is contextual and none is shown: no command bar, and still no toolbar.
        assert!(!flat_toolbar_shown(true, true));
        assert!(flat_toolbar_shown(false, true));
        assert!(flat_toolbar_shown(true, false));
    }

    fn collapsed_after(events: Vec<Vec<egui::Event>>) -> bool {
        let ctx = egui::Context::default();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let look = Look {
            icons: None,
            colors: &colors,
        };
        let id = egui::Id::NULL.with("tabs");
        for events in events {
            let bar = CommandBarView {
                state_id: id,
                heads: ["Home", "Draw"]
                    .iter()
                    .enumerate()
                    .map(|(index, l)| TabHead {
                        index,
                        label: (*l).into(),
                        contextual: false,
                    })
                    .collect(),
                current: 0,
                collapsed: false,
                groups: Vec::new(),
            };
            let raw = egui::RawInput {
                events,
                ..egui::RawInput::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                show_commands(ui, &bar, &look);
            });
            out.textures_delta.clear();
        }
        let state: CommandTabsState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
        state.collapsed
    }

    #[test]
    fn the_panel_folds_from_the_keyboard() {
        let none = egui::Modifiers::NONE;
        assert!(collapsed_after(vec![vec![key(
            egui::Key::F1,
            egui::Modifiers::COMMAND
        )]]));
        // Tab focuses the first tab (the current one), Enter folds.
        assert!(collapsed_after(vec![
            vec![],
            vec![key(egui::Key::Tab, none)],
            vec![key(egui::Key::Enter, none)],
        ]));
        assert!(!collapsed_after(vec![
            vec![],
            vec![key(egui::Key::Enter, none)]
        ]));
    }

    #[test]
    fn selecting_a_tab_expands_a_collapsed_panel() {
        let mut state = CommandTabsState {
            collapsed: true,
            ..CommandTabsState::default()
        };
        state.select(1, false);
        assert!(!state.collapsed);
    }

    fn icon_view(icon: &str, size: ButtonSize) -> ToolView {
        ToolView::Icon {
            label: "Line".into(),
            hint: Some("Draws a line".into()),
            line: "LINE".into(),
            icon: icon.into(),
            size,
        }
    }

    /// Clicks the first widget `draw` adds: moves the pointer over it, presses and releases.
    fn click_first(ctx: &egui::Context, mut draw: impl FnMut(&mut Ui)) {
        let rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let run = |events: Vec<egui::Event>, draw: &mut dyn FnMut(&mut Ui)| {
            let raw = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..egui::RawInput::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                draw(ui);
                if rect.get() == egui::Rect::NOTHING {
                    rect.set(ui.min_rect());
                }
            });
            out.textures_delta.clear();
        };
        run(Vec::new(), &mut draw);
        let at = rect.get().center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run(vec![egui::Event::PointerMoved(at)], &mut draw);
        run(vec![button(true)], &mut draw);
        run(vec![button(false)], &mut draw);
    }

    #[test]
    fn an_icon_button_submits_its_line_with_or_without_the_icon() {
        let ctx = egui::Context::default();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let icons = arc_ui::IconAtlas::build(arc_ui::icons::STARTER, 32)
            .unwrap()
            .upload(&ctx);
        for (name, icons) in [
            ("plus", Some(&icons)),
            ("nonexistent", Some(&icons)),
            ("plus", None),
        ] {
            for size in [ButtonSize::Large, ButtonSize::Small] {
                for quick in [false, true] {
                    let look = Look {
                        icons,
                        colors: &colors,
                    };
                    let view = icon_view(name, size);
                    let mut reply = None;
                    click_first(&ctx, |ui| draw_view(ui, &view, &look, quick, &mut reply));
                    assert_eq!(
                        reply,
                        Some(ToolReply::Line("LINE".into())),
                        "{name} {size:?} {quick}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_large_button_is_wide_enough_for_its_longest_word() {
        let ctx = egui::Context::default();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let look = Look {
            icons: None,
            colors: &colors,
        };
        let width = |label: &str| {
            let view = ToolView::Icon {
                label: label.into(),
                hint: None,
                line: "LINE".into(),
                icon: "plus".into(),
                size: ButtonSize::Large,
            };
            let mut used = 0.0;
            let raw = egui::RawInput::default();
            let mut out = ctx.run_ui(raw, |ui| {
                let mut reply = None;
                ui.horizontal(|ui| draw_view(ui, &view, &look, false, &mut reply));
                used = ui.min_rect().width();
            });
            out.textures_delta.clear();
            used
        };
        assert!((width("Wall") - LARGE_W).abs() < 0.5);
        assert!(width("Uncharacteristically long") > LARGE_W + 20.0);
    }

    #[test]
    fn a_zoom_extents_icon_button_requests_the_zoom() {
        let ctx = egui::Context::default();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let icons = arc_ui::IconAtlas::build(arc_ui::icons::STARTER, 32)
            .unwrap()
            .upload(&ctx);
        let look = Look {
            icons: Some(&icons),
            colors: &colors,
        };
        let view = ToolView::ZoomIcon {
            label: "Zoom".into(),
            hint: None,
            icon: "plus".into(),
            size: ButtonSize::Small,
        };
        let mut reply = None;
        click_first(&ctx, |ui| draw_view(ui, &view, &look, false, &mut reply));
        assert_eq!(reply, Some(ToolReply::Shell(ShellAction::ZoomExtents)));
    }

    #[test]
    fn an_icon_button_is_a_labeled_button_for_assistive_technology() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let colors = arc_ui::ThemeId::Dark.theme().colors;
        let look = Look {
            icons: None,
            colors: &colors,
        };
        let view = icon_view("plus", ButtonSize::Large);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw_view(ui, &view, &look, false, &mut None);
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.unwrap();
        assert!(update.nodes.iter().any(|(_, node)| {
            node.role() == egui::accesskit::Role::Button && node.label() == Some("Line")
        }));
    }

    #[test]
    fn view_menu_rows_follow_the_layout() {
        let default = layout();
        let mut current = default.clone();
        current.close("properties").unwrap();
        let panels: Vec<(String, String)> = ["viewport", "properties"]
            .iter()
            .map(|p| ((*p).to_owned(), p.to_uppercase()))
            .collect();
        let rows = panel_rows(&current, &panels);
        assert_eq!(
            rows.iter().map(|r| r.shown).collect::<Vec<_>>(),
            [true, false]
        );
    }
}
