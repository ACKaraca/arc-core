//! The window chrome around the dock: the View menu (which panels are shown, reopening a closed
//! panel, resetting the layout), the toolbar strip, the status bar, and the keyboard routing that
//! sends typing outside a text field to the command line. The decisions are pure functions so
//! tests run them without a window; the drawing is thin.

use std::sync::Arc;

use arc_base::error::Value;
use arc_ui::egui::{self, Ui};
use arc_ui::{
    Axis, Dock, DockLayout, Intent, LineAction, Localize, Message, Side, Target, resolve,
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
    /// A gap between groups.
    Separator,
}

impl std::fmt::Debug for ToolItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Button { label, line, .. } => write!(f, "Button({label}, {line})"),
            Self::Choice { .. } => f.write_str("Choice"),
            Self::ZoomExtents { label } => write!(f, "ZoomExtents({label})"),
            Self::Separator => f.write_str("Separator"),
        }
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
    /// The message source.
    pub localize: &'a dyn Localize,
}

/// Draws the menu bar (with the View menu), the toolbar strip and the status bar around the
/// dock; what the user chose becomes actions.
pub fn show_frame(ui: &mut Ui, frame: &ChromeFrame<'_>, actions: &mut Vec<Action>) {
    egui::Panel::top("arc-app-menu").show(ui, |ui| {
        let (intent, chosen) = arc_ui::menu::show_menu_bar_with(ui, frame.entries, |ui| {
            view_menu(ui, frame.rows, frame.localize)
        });
        actions.extend(intent.map(Action::Start));
        actions.extend(chosen.map(Action::Shell));
    });
    if !frame.tools.is_empty() {
        egui::Panel::top("arc-app-tools").show(ui, |ui| match show_tools(ui, frame.tools) {
            Some(ToolReply::Line(line)) => actions.push(Action::Line(LineAction::Submit(line))),
            // Through the line: a product's controller runs it with the parameters given,
            // without prompting for them.
            Some(ToolReply::Intent(intent)) => {
                actions.push(Action::Line(LineAction::Invoke(intent)));
            }
            Some(ToolReply::Shell(action)) => actions.push(Action::Shell(action)),
            None => {}
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
        if let Some(intent) = show_status(ui, frame.cursor, frame.toggles) {
            actions.push(Action::Line(LineAction::Invoke(intent)));
        }
    });
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
    ui.horizontal_wrapped(|ui| {
        for tab in tabs {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let label = tab_label(tab);
                if ui.selectable_label(tab.active, label).clicked() && !tab.active {
                    chosen = Some(ShellAction::ActivateTab(tab.id));
                }
                let close = ui
                    .small_button("\u{d7}")
                    .on_hover_text(TAB_CLOSE.text(localize));
                if close.clicked() {
                    chosen = Some(ShellAction::CloseTab(tab.id));
                }
            });
            ui.add_space(6.0);
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
            ToolItem::Choice { hint, probe, pick } => ToolView::Choice {
                hint: hint.as_deref().map(text),
                state: tab.map(|t| probe(app, t)).unwrap_or_default(),
                pick: Arc::clone(pick),
            },
            ToolItem::ZoomExtents { label } => ToolView::Zoom { label: text(label) },
            ToolItem::Separator => ToolView::Separator,
        })
        .collect()
}

/// Draws the toolbar strip and returns what the user clicked.
pub fn show_tools(ui: &mut Ui, views: &[ToolView]) -> Option<ToolReply> {
    let mut reply = None;
    ui.horizontal_wrapped(|ui| {
        for view in views {
            match view {
                ToolView::Button { label, hint, line } => {
                    let mut response = ui.button(label.as_str());
                    if let Some(hint) = hint {
                        response = response.on_hover_text(hint.as_str());
                    }
                    if response.clicked() {
                        reply = Some(ToolReply::Line(line.clone()));
                    }
                }
                ToolView::Zoom { label } => {
                    if ui.button(label.as_str()).clicked() {
                        reply = Some(ToolReply::Shell(ShellAction::ZoomExtents));
                    }
                }
                ToolView::Choice { hint, state, pick } => {
                    let current = state.current.clone().unwrap_or_default();
                    let combo = egui::ComboBox::from_id_salt("arc-toolbar-choice")
                        .selected_text(current.as_str())
                        .show_ui(ui, |ui| {
                            for option in &state.options {
                                let selected = state.current.as_deref() == Some(option.as_str());
                                if ui.selectable_label(selected, option.as_str()).clicked()
                                    && !selected
                                {
                                    reply = Some(ToolReply::Intent(pick(option)));
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
    });
    reply
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

/// Draws the status bar content: the cursor coordinates on the left, the toggles after them.
/// Returns the intent of a toggled mode.
pub fn show_status(ui: &mut Ui, cursor: &str, toggles: &[ToggleView]) -> Option<Intent> {
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
