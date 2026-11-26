//! Docking (CORE-UI-001): a layout of tab groups and splits as validated data, operations on it,
//! and a widget that draws it.
//!
//! A [`DockLayout`] has a main tree and up to [`MAX_FLOATING`] floating trees. A tree is a
//! [`Dock`]: a tab group of panel identifiers, or a split of two trees along an axis. The
//! serialized form is a flat node list with indices (no recursion while decoding); converting it
//! checks that the nodes form trees within [`MAX_DEPTH`] levels, [`MAX_PANELS`] panels and
//! [`MAX_FLOATING`] floating trees, with each panel at most once. Operations work on a copy,
//! normalize it (empty groups are removed, a split with one child is replaced by that child) and
//! check it again, so a failed operation leaves the layout unchanged.
//!
//! Floating trees are data only here; native windows for them come with the application shell.
//! [`DockArea`] draws the main tree: tab bars, panel content, separators that pointer drag or
//! arrow keys move, a close button per tab, and dropping a dragged tab on a group's center or
//! edge to join or split it.

use arc_base::{Error, Result};
use egui::{Key, Modifiers, Rect, Role, Sense, Ui, UiBuilder, WidgetInfo, vec2};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::text::{Direction, Localize, messages};

/// The most panels in one layout, over all trees.
pub const MAX_PANELS: usize = 256;
/// The deepest nesting of splits in one tree; a lone tab group has depth 0.
pub const MAX_DEPTH: usize = 16;
/// The most floating trees.
pub const MAX_FLOATING: usize = 32;
/// The most nodes in the serialized form.
pub const MAX_NODES: usize = 2 * MAX_PANELS + MAX_FLOATING + 1;
/// The longest panel identifier, in bytes.
pub const MAX_PANEL_ID_BYTES: usize = 128;
/// The smallest share of a split's first part; the largest is `1 - MIN_FRACTION`.
pub const MIN_FRACTION: f32 = 0.05;
/// The largest coordinate of a floating window, in logical points.
pub const MAX_COORDINATE: f32 = 1.0e6;

/// The direction a split divides its area in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Side by side: the first part on the left (the logical start).
    Horizontal,
    /// Stacked: the first part on top.
    Vertical,
}

/// A side of a group or of the main area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The left (logical start).
    Left,
    /// The right (logical end).
    Right,
    /// The top.
    Top,
    /// The bottom.
    Bottom,
}

/// A tree of tab groups and splits.
#[derive(Debug, Clone, PartialEq)]
pub enum Dock {
    /// A tab group.
    Tabs {
        /// Panel identifiers in tab order; only the main tree's root may be empty.
        tabs: Vec<String>,
        /// The index of the shown tab.
        active: usize,
    },
    /// Two trees side by side or stacked.
    Split {
        /// The direction.
        axis: Axis,
        /// The first part's share, in `[MIN_FRACTION, 1 - MIN_FRACTION]`.
        fraction: f32,
        /// The left or top part.
        first: Box<Dock>,
        /// The right or bottom part.
        second: Box<Dock>,
    },
}

/// A floating tree with its window rectangle `[x, y, width, height]` in logical points.
#[derive(Debug, Clone, PartialEq)]
pub struct Floating {
    /// The tree.
    pub dock: Dock,
    /// The window rectangle.
    pub rect: [f32; 4],
}

/// Where an operation puts a panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Into the group that holds `anchor`, at `index` (clamped; `None` appends).
    Tab {
        /// A panel of the group.
        anchor: String,
        /// The tab position.
        index: Option<usize>,
    },
    /// Into a new group on `side` of the group that holds `anchor`.
    Split {
        /// A panel of the group.
        anchor: String,
        /// The side.
        side: Side,
    },
    /// Into a new group on `side` of the whole main tree, or into the main group when it is
    /// empty.
    Root {
        /// The side.
        side: Side,
    },
    /// Into a new floating tree.
    Float {
        /// The window rectangle `[x, y, width, height]`.
        rect: [f32; 4],
    },
}

/// A validated dock layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LayoutRaw", into = "LayoutRaw")]
pub struct DockLayout {
    root: Dock,
    floating: Vec<Floating>,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            root: Dock::Tabs {
                tabs: Vec::new(),
                active: 0,
            },
            floating: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NodeRaw {
    Tabs {
        tabs: Vec<String>,
        #[serde(default)]
        active: usize,
    },
    Split {
        axis: Axis,
        fraction: f32,
        first: usize,
        second: usize,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FloatingRaw {
    root: usize,
    rect: [f32; 4],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutRaw {
    root: usize,
    nodes: Vec<NodeRaw>,
    #[serde(default)]
    floating: Vec<FloatingRaw>,
}

fn invalid(reason: &'static str) -> Error {
    Error::new(codes::DOCK_INVALID).with_detail("reason", reason)
}

fn too_large(what: &'static str) -> Error {
    Error::new(codes::DOCK_TOO_LARGE).with_detail("limit", what)
}

/// True when `id` is a panel identifier: 1 to [`MAX_PANEL_ID_BYTES`] bytes of ASCII letters,
/// digits, `.`, `-` and `_`, starting with a letter.
#[must_use]
pub fn is_panel_id(id: &str) -> bool {
    id.len() <= MAX_PANEL_ID_BYTES
        && id.starts_with(|c: char| c.is_ascii_alphabetic())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn check_rect(rect: [f32; 4]) -> Result<()> {
    let finite = rect
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE);
    if finite && rect[2] > 0.0 && rect[3] > 0.0 {
        Ok(())
    } else {
        Err(invalid("rect"))
    }
}

fn fraction_ok(fraction: f32) -> bool {
    (MIN_FRACTION..=1.0 - MIN_FRACTION).contains(&fraction)
}

/// Checks one tree; `panels` collects its panel identifiers. Recursion is bounded by
/// [`MAX_DEPTH`] + 1 levels.
fn check_tree(
    dock: &Dock,
    depth: usize,
    may_be_empty: bool,
    panels: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::new(codes::DOCK_TOO_DEEP));
    }
    match dock {
        Dock::Tabs { tabs, active } => {
            if tabs.is_empty() && !(may_be_empty && depth == 0) {
                return Err(invalid("empty_group"));
            }
            if *active >= tabs.len().max(1) {
                return Err(invalid("active"));
            }
            for tab in tabs {
                if !is_panel_id(tab) {
                    return Err(invalid("panel_id"));
                }
                if panels.len() >= MAX_PANELS {
                    return Err(too_large("panels"));
                }
                if !panels.insert(tab.clone()) {
                    return Err(
                        Error::new(codes::DOCK_PANEL_DUPLICATE).with_arg("panel", tab.as_str())
                    );
                }
            }
            Ok(())
        }
        Dock::Split {
            fraction,
            first,
            second,
            ..
        } => {
            if !fraction_ok(*fraction) {
                return Err(invalid("fraction"));
            }
            check_tree(first, depth + 1, false, panels)?;
            check_tree(second, depth + 1, false, panels)
        }
    }
}

/// Removes empty groups and splits with one child; `None` when nothing is left.
fn normalize(dock: Dock) -> Option<Dock> {
    match dock {
        Dock::Tabs { tabs, active } if !tabs.is_empty() => {
            let active = active.min(tabs.len() - 1);
            Some(Dock::Tabs { tabs, active })
        }
        Dock::Tabs { .. } => None,
        Dock::Split {
            axis,
            fraction,
            first,
            second,
        } => match (normalize(*first), normalize(*second)) {
            (Some(first), Some(second)) => Some(Dock::Split {
                axis,
                fraction,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
    }
}

/// The group holding `panel`, mutably.
fn group_of<'a>(dock: &'a mut Dock, panel: &str) -> Option<&'a mut Dock> {
    match dock {
        Dock::Tabs { tabs, .. } => tabs.iter().any(|t| t == panel).then_some(dock),
        Dock::Split { first, second, .. } => {
            if contains(first, panel) {
                group_of(first, panel)
            } else {
                group_of(second, panel)
            }
        }
    }
}

fn contains(dock: &Dock, panel: &str) -> bool {
    match dock {
        Dock::Tabs { tabs, .. } => tabs.iter().any(|t| t == panel),
        Dock::Split { first, second, .. } => contains(first, panel) || contains(second, panel),
    }
}

fn collect(dock: &Dock, out: &mut Vec<String>) {
    match dock {
        Dock::Tabs { tabs, .. } => out.extend(tabs.iter().cloned()),
        Dock::Split { first, second, .. } => {
            collect(first, out);
            collect(second, out);
        }
    }
}

fn single(panel: &str) -> Dock {
    Dock::Tabs {
        tabs: vec![panel.to_owned()],
        active: 0,
    }
}

/// Puts `new` on `side` of `old`.
fn split_beside(old: Dock, new: Dock, side: Side) -> Dock {
    let (axis, new_first) = match side {
        Side::Left => (Axis::Horizontal, true),
        Side::Right => (Axis::Horizontal, false),
        Side::Top => (Axis::Vertical, true),
        Side::Bottom => (Axis::Vertical, false),
    };
    let (first, second) = if new_first { (new, old) } else { (old, new) };
    Dock::Split {
        axis,
        fraction: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// A name no panel can have, marking a panel's old place while it moves.
const MOVING: &str = "";

fn rename(dock: &mut Dock, from: &str, to: &str) {
    if let Some(Dock::Tabs { tabs, .. }) = group_of(dock, from) {
        for tab in tabs.iter_mut().filter(|t| *t == from) {
            *tab = to.to_owned();
        }
    }
}

fn remove(dock: &mut Dock, panel: &str) {
    if let Some(Dock::Tabs { tabs, active }) = group_of(dock, panel)
        && let Some(at) = tabs.iter().position(|t| t == panel)
    {
        tabs.remove(at);
        if at < *active || *active >= tabs.len() {
            *active = active.saturating_sub(1);
        }
    }
}

impl DockLayout {
    /// A layout of a main tree and floating trees.
    ///
    /// # Errors
    ///
    /// `ui.dock.invalid` for an empty group other than the main root, an active index out of
    /// range, a malformed panel identifier, a fraction outside the range or a malformed window
    /// rectangle; `ui.dock.too_deep` beyond [`MAX_DEPTH`]; `ui.dock.too_large` beyond
    /// [`MAX_PANELS`] or [`MAX_FLOATING`]; `ui.dock.panel_duplicate` for a panel shown twice.
    pub fn new(root: Dock, floating: Vec<Floating>) -> Result<Self> {
        let layout = Self { root, floating };
        layout.check()?;
        Ok(layout)
    }

    fn check(&self) -> Result<()> {
        if self.floating.len() > MAX_FLOATING {
            return Err(too_large("floating"));
        }
        let mut panels = std::collections::BTreeSet::new();
        check_tree(&self.root, 0, true, &mut panels)?;
        for window in &self.floating {
            check_rect(window.rect)?;
            check_tree(&window.dock, 0, false, &mut panels)?;
        }
        Ok(())
    }

    /// The main tree.
    #[must_use]
    pub fn root(&self) -> &Dock {
        &self.root
    }

    /// The floating trees.
    #[must_use]
    pub fn floating(&self) -> &[Floating] {
        &self.floating
    }

    /// Every panel, the main tree first, each tree from left and top to right and bottom.
    #[must_use]
    pub fn panels(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        for window in &self.floating {
            collect(&window.dock, &mut out);
        }
        out
    }

    /// True when the layout shows `panel`.
    #[must_use]
    pub fn contains(&self, panel: &str) -> bool {
        contains(&self.root, panel) || self.floating.iter().any(|w| contains(&w.dock, panel))
    }

    fn tree_of(&mut self, panel: &str) -> Option<&mut Dock> {
        if contains(&self.root, panel) {
            return Some(&mut self.root);
        }
        self.floating
            .iter_mut()
            .map(|w| &mut w.dock)
            .find(|d| contains(d, panel))
    }

    /// Applies `change` to a copy, normalizes and checks it, and keeps it only when valid.
    fn apply(&mut self, change: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let mut next = self.clone();
        change(&mut next)?;
        let root = std::mem::replace(&mut next.root, single(MOVING));
        next.root = normalize(root).unwrap_or(Dock::Tabs {
            tabs: Vec::new(),
            active: 0,
        });
        next.floating = std::mem::take(&mut next.floating)
            .into_iter()
            .filter_map(|w| normalize(w.dock).map(|dock| Floating { dock, rect: w.rect }))
            .collect();
        next.check()?;
        *self = next;
        Ok(())
    }

    fn insert(&mut self, panel: &str, target: &Target) -> Result<()> {
        let unknown = |anchor: &str| {
            Error::new(codes::DOCK_PANEL_UNKNOWN).with_arg("panel", anchor.to_owned())
        };
        match target {
            Target::Tab { anchor, index } => {
                let anchor = if anchor == panel { MOVING } else { anchor };
                let tree = self.tree_of(anchor).ok_or_else(|| unknown(anchor))?;
                if let Some(Dock::Tabs { tabs, active }) = group_of(tree, anchor) {
                    let at = index.unwrap_or(tabs.len()).min(tabs.len());
                    tabs.insert(at, panel.to_owned());
                    *active = at;
                }
            }
            Target::Split { anchor, side } => {
                let anchor = if anchor == panel { MOVING } else { anchor };
                let tree = self.tree_of(anchor).ok_or_else(|| unknown(anchor))?;
                if let Some(group) = group_of(tree, anchor) {
                    let old = std::mem::replace(group, single(MOVING));
                    *group = split_beside(old, single(panel), *side);
                }
            }
            Target::Root { side } => {
                if let Dock::Tabs { tabs, .. } = &mut self.root
                    && tabs.is_empty()
                {
                    tabs.push(panel.to_owned());
                } else {
                    let old = std::mem::replace(&mut self.root, single(MOVING));
                    self.root = split_beside(old, single(panel), *side);
                }
            }
            Target::Float { rect } => {
                check_rect(*rect)?;
                self.floating.push(Floating {
                    dock: single(panel),
                    rect: *rect,
                });
            }
        }
        Ok(())
    }

    /// Adds `panel` at `target`.
    ///
    /// # Errors
    ///
    /// `ui.dock.panel_duplicate` when the layout already shows it, `ui.dock.panel_unknown` for
    /// an anchor it does not show, and the errors of [`DockLayout::new`] for the result.
    pub fn add(&mut self, panel: &str, target: &Target) -> Result<()> {
        if self.contains(panel) {
            return Err(Error::new(codes::DOCK_PANEL_DUPLICATE).with_arg("panel", panel.to_owned()));
        }
        self.apply(|next| next.insert(panel, target))
    }

    /// Moves `panel` to `target`; the anchor may be the panel itself, meaning its current group.
    ///
    /// # Errors
    ///
    /// `ui.dock.panel_unknown` for a panel or anchor the layout does not show, and the errors of
    /// [`DockLayout::new`] for the result.
    pub fn move_panel(&mut self, panel: &str, target: &Target) -> Result<()> {
        self.known(panel)?;
        self.apply(|next| {
            let tree = next.tree_of(panel).ok_or_else(|| invalid("missing"))?;
            rename(tree, panel, MOVING);
            next.insert(panel, target)?;
            let tree = next.tree_of(MOVING).ok_or_else(|| invalid("missing"))?;
            remove(tree, MOVING);
            Ok(())
        })
    }

    fn known(&self, panel: &str) -> Result<()> {
        if self.contains(panel) {
            Ok(())
        } else {
            Err(Error::new(codes::DOCK_PANEL_UNKNOWN).with_arg("panel", panel.to_owned()))
        }
    }

    /// Removes `panel`; an emptied group or floating tree goes away.
    ///
    /// # Errors
    ///
    /// `ui.dock.panel_unknown` when the layout does not show it.
    pub fn close(&mut self, panel: &str) -> Result<()> {
        self.known(panel)?;
        self.apply(|next| {
            if let Some(tree) = next.tree_of(panel) {
                remove(tree, panel);
            }
            Ok(())
        })
    }

    /// Makes `panel` the shown tab of its group.
    ///
    /// # Errors
    ///
    /// `ui.dock.panel_unknown` when the layout does not show it.
    pub fn activate(&mut self, panel: &str) -> Result<()> {
        self.known(panel)?;
        if let Some(Dock::Tabs { tabs, active }) =
            self.tree_of(panel).and_then(|t| group_of(t, panel))
            && let Some(at) = tabs.iter().position(|t| t == panel)
        {
            *active = at;
        }
        Ok(())
    }

    /// Moves `panel` into a new floating tree.
    ///
    /// # Errors
    ///
    /// Those of [`DockLayout::move_panel`].
    pub fn float(&mut self, panel: &str, rect: [f32; 4]) -> Result<()> {
        self.move_panel(panel, &Target::Float { rect })
    }

    /// Moves `panel` from wherever it is into the main tree, on the right of it.
    ///
    /// # Errors
    ///
    /// Those of [`DockLayout::move_panel`].
    pub fn unfloat(&mut self, panel: &str) -> Result<()> {
        self.move_panel(panel, &Target::Root { side: Side::Right })
    }

    /// Sets the fraction of the split of the main tree reached by `path` (`false` steps into the
    /// first part, `true` into the second), clamped into the valid range. A path that does not
    /// end at a split changes nothing.
    pub fn set_fraction(&mut self, path: &[bool], fraction: f32) {
        let mut dock = &mut self.root;
        for &second in path {
            match dock {
                Dock::Split {
                    first, second: s, ..
                } => dock = if second { s } else { first },
                Dock::Tabs { .. } => return,
            }
        }
        if let Dock::Split { fraction: f, .. } = dock
            && fraction.is_finite()
        {
            *f = fraction.clamp(MIN_FRACTION, 1.0 - MIN_FRACTION);
        }
    }
}

/// Appends `dock`'s nodes after converting its children; recursion is bounded by the checked
/// depth.
fn flatten(dock: &Dock, nodes: &mut Vec<NodeRaw>) -> usize {
    match dock {
        Dock::Tabs { tabs, active } => {
            nodes.push(NodeRaw::Tabs {
                tabs: tabs.clone(),
                active: *active,
            });
        }
        Dock::Split {
            axis,
            fraction,
            first,
            second,
        } => {
            let first = flatten(first, nodes);
            let second = flatten(second, nodes);
            nodes.push(NodeRaw::Split {
                axis: *axis,
                fraction: *fraction,
                first,
                second,
            });
        }
    }
    nodes.len() - 1
}

impl From<DockLayout> for LayoutRaw {
    fn from(layout: DockLayout) -> Self {
        let mut nodes = Vec::new();
        let root = flatten(&layout.root, &mut nodes);
        let floating = layout
            .floating
            .iter()
            .map(|w| FloatingRaw {
                root: flatten(&w.dock, &mut nodes),
                rect: w.rect,
            })
            .collect();
        Self {
            root,
            nodes,
            floating,
        }
    }
}

/// Builds the tree at `index`; called only after the node graph is known to be a forest within
/// the depth limit.
fn build(nodes: &mut [Option<NodeRaw>], index: usize) -> Result<Dock> {
    match nodes.get_mut(index).and_then(Option::take) {
        Some(NodeRaw::Tabs { tabs, active }) => Ok(Dock::Tabs { tabs, active }),
        Some(NodeRaw::Split {
            axis,
            fraction,
            first,
            second,
        }) => Ok(Dock::Split {
            axis,
            fraction,
            first: Box::new(build(nodes, first)?),
            second: Box::new(build(nodes, second)?),
        }),
        None => Err(invalid("reference")),
    }
}

impl TryFrom<LayoutRaw> for DockLayout {
    type Error = Error;

    fn try_from(raw: LayoutRaw) -> Result<Self> {
        if raw.nodes.len() > MAX_NODES {
            return Err(too_large("nodes"));
        }
        if raw.floating.len() > MAX_FLOATING {
            return Err(too_large("floating"));
        }
        // Every node is referenced exactly once (as a root or a child) and reached from a root
        // within the depth limit: then the nodes form trees and building cannot loop.
        let mut seen = vec![false; raw.nodes.len()];
        let roots = std::iter::once(raw.root).chain(raw.floating.iter().map(|w| w.root));
        let mut stack: Vec<(usize, usize)> = roots.map(|r| (r, 0)).collect();
        while let Some((index, depth)) = stack.pop() {
            if depth > MAX_DEPTH {
                return Err(Error::new(codes::DOCK_TOO_DEEP));
            }
            match seen.get_mut(index) {
                Some(slot) if !*slot => *slot = true,
                _ => return Err(invalid("reference")),
            }
            if let Some(NodeRaw::Split { first, second, .. }) = raw.nodes.get(index) {
                stack.push((*second, depth + 1));
                stack.push((*first, depth + 1));
            }
        }
        if seen.iter().any(|s| !s) {
            return Err(invalid("unreachable"));
        }
        let mut nodes: Vec<Option<NodeRaw>> = raw.nodes.into_iter().map(Some).collect();
        let root = build(&mut nodes, raw.root)?;
        let floating = raw
            .floating
            .iter()
            .map(|w| {
                Ok(Floating {
                    dock: build(&mut nodes, w.root)?,
                    rect: w.rect,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(root, floating)
    }
}

/// The panel a tab drag carries.
#[derive(Debug, Clone)]
struct TabDrag(String);

/// Draws the main tree of a [`DockLayout`].
#[derive(Debug, Clone, Copy)]
pub struct DockArea {
    /// The width of the separators between split parts, in logical points.
    pub separator: f32,
    /// The layout direction. Right to left puts the first part of a horizontal split at the
    /// right and orders the tabs of a group from the right (CORE-UI-025).
    pub direction: Direction,
}

impl Default for DockArea {
    fn default() -> Self {
        Self {
            separator: 6.0,
            direction: Direction::LeftToRight,
        }
    }
}

/// The step a focused separator moves by per arrow key, as a fraction.
const KEY_STEP: f32 = 0.05;

impl DockArea {
    /// Draws `layout` into the available rectangle. `title` gives a panel's localized title and
    /// `content` draws a panel's content. Tab clicks, close buttons, separator drags and arrow
    /// keys, and tab drops change `layout` directly: the layout is user interface state, not
    /// document data.
    pub fn show(
        self,
        ui: &mut Ui,
        layout: &mut DockLayout,
        localize: &dyn Localize,
        title: &dyn Fn(&str) -> String,
        content: &mut dyn FnMut(&mut Ui, &str),
    ) {
        let rect = ui.available_rect_before_wrap();
        // The root `Ui` of a pass paints no background, so the dock fills its own area.
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        let mut changes = Vec::new();
        let root = layout.root.clone();
        let mut path = Vec::new();
        self.draw(
            ui,
            &root,
            rect,
            &mut path,
            localize,
            title,
            content,
            &mut changes,
        );
        ui.allocate_rect(rect, Sense::hover());
        for change in changes {
            // A change that no longer applies (the panel moved meanwhile) is dropped.
            let _ = match change {
                Change::Activate(panel) => layout.activate(&panel),
                Change::Close(panel) => layout.close(&panel),
                Change::Move(panel, target) => layout.move_panel(&panel, &target),
                Change::Fraction(path, f) => {
                    layout.set_fraction(&path, f);
                    Ok(())
                }
            };
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one recursive drawing step")]
    fn draw(
        self,
        ui: &mut Ui,
        dock: &Dock,
        rect: Rect,
        path: &mut Vec<bool>,
        localize: &dyn Localize,
        title: &dyn Fn(&str) -> String,
        content: &mut dyn FnMut(&mut Ui, &str),
        changes: &mut Vec<Change>,
    ) {
        match dock {
            Dock::Split {
                axis,
                fraction,
                first,
                second,
            } => {
                let reversed = self.direction.is_right_to_left() && *axis == Axis::Horizontal;
                let (a, sep, b) = divide(rect, *axis, *fraction, self.separator, reversed);
                self.draw_child(ui, first, a, path, false, localize, title, content, changes);
                let id = ui.make_persistent_id(("arc-dock-separator", path.clone()));
                let response = ui.interact(sep, id, Sense::drag());
                response.widget_info(|| {
                    WidgetInfo::labeled(
                        Role::Splitter,
                        true,
                        messages::DOCK_SEPARATOR.text(localize),
                    )
                });
                let length = match axis {
                    Axis::Horizontal => rect.width(),
                    Axis::Vertical => rect.height(),
                };
                let mut next = None;
                if response.dragged() && length > 0.0 {
                    let delta = response.drag_delta();
                    let moved = match axis {
                        // In a reversed split the first part is at the right: dragging right
                        // shrinks it.
                        Axis::Horizontal if reversed => -delta.x,
                        Axis::Horizontal => delta.x,
                        Axis::Vertical => delta.y,
                    };
                    next = Some(fraction + moved / length);
                }
                if response.has_focus() {
                    let (back, forward) = match axis {
                        Axis::Horizontal if reversed => (Key::ArrowRight, Key::ArrowLeft),
                        Axis::Horizontal => (Key::ArrowLeft, Key::ArrowRight),
                        Axis::Vertical => (Key::ArrowUp, Key::ArrowDown),
                    };
                    let (minus, plus) = ui.input_mut(|i| {
                        (
                            i.consume_key(Modifiers::NONE, back),
                            i.consume_key(Modifiers::NONE, forward),
                        )
                    });
                    if minus {
                        next = Some(fraction - KEY_STEP);
                    } else if plus {
                        next = Some(fraction + KEY_STEP);
                    }
                }
                let visuals = ui.style().interact(&response);
                ui.painter().rect_filled(sep, 0.0, visuals.bg_fill);
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        sep,
                        0.0,
                        ui.visuals().selection.stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(f) = next {
                    changes.push(Change::Fraction(path.clone(), f));
                }
                self.draw_child(ui, second, b, path, true, localize, title, content, changes);
            }
            Dock::Tabs { tabs, active } => {
                self.draw_group(ui, tabs, *active, rect, localize, title, content, changes);
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one recursive drawing step")]
    fn draw_child(
        self,
        ui: &mut Ui,
        dock: &Dock,
        rect: Rect,
        path: &mut Vec<bool>,
        second: bool,
        localize: &dyn Localize,
        title: &dyn Fn(&str) -> String,
        content: &mut dyn FnMut(&mut Ui, &str),
        changes: &mut Vec<Change>,
    ) {
        path.push(second);
        self.draw(ui, dock, rect, path, localize, title, content, changes);
        path.pop();
    }

    #[allow(clippy::too_many_arguments, reason = "one drawing step")]
    fn draw_group(
        self,
        ui: &mut Ui,
        tabs: &[String],
        active: usize,
        rect: Rect,
        localize: &dyn Localize,
        title: &dyn Fn(&str) -> String,
        content: &mut dyn FnMut(&mut Ui, &str),
        changes: &mut Vec<Change>,
    ) {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        // Each group paints its own background and a visible border.
        child
            .painter()
            .rect_filled(rect, 0.0, child.visuals().panel_fill);
        child.painter().rect_stroke(
            rect.shrink(0.5),
            0.0,
            child.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
        // A header bar behind the tabs, set off from the panel body by a line.
        let header = Rect::from_min_size(
            rect.min,
            vec2(rect.width(), child.spacing().interact_size.y),
        );
        child
            .painter()
            .rect_filled(header, 0.0, child.visuals().extreme_bg_color);
        child.painter().hline(
            header.x_range(),
            header.bottom(),
            child.visuals().widgets.noninteractive.bg_stroke,
        );
        let bar = self.direction.row(&mut child, false, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, panel) in tabs.iter().enumerate() {
                let name = title(panel);
                let close = messages::DOCK_CLOSE.text(localize);
                let (tab, x) = flat_tab(ui, &name, &name, active == index, &close, true);
                if tab.clicked() {
                    changes.push(Change::Activate(panel.clone()));
                }
                if tab.drag_started() {
                    tab.dnd_set_drag_payload(TabDrag(panel.clone()));
                }
                if x.clicked() {
                    changes.push(Change::Close(panel.clone()));
                }
            }
        });
        let body = Rect::from_min_max(
            egui::pos2(rect.left(), bar.response.rect.bottom()),
            rect.max,
        );
        if let Some(panel) = tabs.get(active) {
            let mut inner = child.new_child(UiBuilder::new().max_rect(body.shrink(2.0)));
            inner.set_clip_rect(body.shrink(1.0).intersect(child.clip_rect()));
            content(&mut inner, panel);
        }
        let Some(anchor) = tabs.first() else {
            return;
        };
        let drop = ui.interact(
            body,
            ui.make_persistent_id(("arc-dock-drop", anchor)),
            Sense::hover(),
        );
        if let Some(pos) = drop.hover_pos()
            && egui::DragAndDrop::has_payload_of_type::<TabDrag>(ui.ctx())
        {
            let zone = drop_zone(body, pos);
            ui.painter().rect_filled(
                zone_rect(body, zone),
                0.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.4),
            );
        }
        if let (Some(payload), Some(pos)) =
            (drop.dnd_release_payload::<TabDrag>(), drop.hover_pos())
        {
            let target = match drop_zone(body, pos) {
                None => Target::Tab {
                    anchor: anchor.clone(),
                    index: None,
                },
                Some(side) => Target::Split {
                    anchor: anchor.clone(),
                    side,
                },
            };
            changes.push(Change::Move(payload.0.clone(), target));
        }
    }
}

enum Change {
    Activate(String),
    Close(String),
    Move(String, Target),
    Fraction(Vec<bool>, f32),
}

/// The two parts and the separator of a split rectangle. The first part is the left one, or the
/// right one when `reversed` (a horizontal split in a right-to-left layout).
fn divide(
    rect: Rect,
    axis: Axis,
    fraction: f32,
    separator: f32,
    reversed: bool,
) -> (Rect, Rect, Rect) {
    let fraction = fraction.clamp(MIN_FRACTION, 1.0 - MIN_FRACTION);
    match axis {
        Axis::Horizontal if reversed => {
            let at = rect.left() + (rect.width() - separator).max(0.0) * (1.0 - fraction);
            (
                Rect::from_min_max(egui::pos2(at + separator, rect.top()), rect.max),
                Rect::from_min_size(egui::pos2(at, rect.top()), vec2(separator, rect.height())),
                Rect::from_min_max(rect.min, egui::pos2(at, rect.bottom())),
            )
        }
        Axis::Horizontal => {
            let at = rect.left() + (rect.width() - separator).max(0.0) * fraction;
            (
                Rect::from_min_max(rect.min, egui::pos2(at, rect.bottom())),
                Rect::from_min_size(egui::pos2(at, rect.top()), vec2(separator, rect.height())),
                Rect::from_min_max(egui::pos2(at + separator, rect.top()), rect.max),
            )
        }
        Axis::Vertical => {
            let at = rect.top() + (rect.height() - separator).max(0.0) * fraction;
            (
                Rect::from_min_max(rect.min, egui::pos2(rect.right(), at)),
                Rect::from_min_size(egui::pos2(rect.left(), at), vec2(rect.width(), separator)),
                Rect::from_min_max(egui::pos2(rect.left(), at + separator), rect.max),
            )
        }
    }
}

/// The edge of `body` a drop at `pos` splits, or `None` for its center (join the group): the
/// outer quarter on each side.
fn drop_zone(body: Rect, pos: egui::Pos2) -> Option<Side> {
    let (w, h) = (body.width().max(1.0), body.height().max(1.0));
    let (x, y) = ((pos.x - body.left()) / w, (pos.y - body.top()) / h);
    let edges = [
        (x, Side::Left),
        (1.0 - x, Side::Right),
        (y, Side::Top),
        (1.0 - y, Side::Bottom),
    ];
    edges
        .into_iter()
        .filter(|(d, _)| *d < 0.25)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, side)| side)
}

fn zone_rect(body: Rect, zone: Option<Side>) -> Rect {
    let (w, h) = (body.width() / 2.0, body.height() / 2.0);
    match zone {
        None => body.shrink2(vec2(w / 2.0, h / 2.0)),
        Some(Side::Left) => Rect::from_min_size(body.min, vec2(w, body.height())),
        Some(Side::Right) => Rect::from_min_size(
            egui::pos2(body.left() + w, body.top()),
            vec2(w, body.height()),
        ),
        Some(Side::Top) => Rect::from_min_size(body.min, vec2(body.width(), h)),
        Some(Side::Bottom) => Rect::from_min_size(
            egui::pos2(body.left(), body.top() + h),
            vec2(body.width(), h),
        ),
    }
}

/// Draws one flat tab of the token row height: `label` (the tab's name, its modified mark
/// included), an accent line on top when `active`, and, when `closable`, an unboxed "×" inside the
/// tab that shows only while the tab is active or hovered (its space is always reserved, so
/// nothing shifts). Returns the tab and the close control; `close` and `name` give the close
/// button its accessible name. Without `closable` the second response never clicks.
pub fn flat_tab(
    ui: &mut Ui,
    label: &str,
    name: &str,
    active: bool,
    close: &str,
    closable: bool,
) -> (egui::Response, egui::Response) {
    let pad = ui.spacing().button_padding.x;
    let height = ui.spacing().interact_size.y;
    let side = height;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let strong = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), strong);
    let close_space = if closable { side } else { 0.0 };
    let size = vec2(pad * 2.0 + galley.size().x + close_space, height);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let tab_rect = Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - close_space, rect.bottom()),
    );
    let tab = ui.interact(
        tab_rect,
        ui.next_auto_id().with(("tab", name)),
        Sense::click_and_drag(),
    );
    let x_rect = Rect::from_center_size(
        egui::pos2(rect.right() - side * 0.5, rect.center().y),
        vec2(side, side),
    );
    let x = ui.interact(
        x_rect,
        tab.id.with("close"),
        if closable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if closable {
        x.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("{close} {name}")));
    }
    tab.widget_info(|| WidgetInfo::selected(Role::Button, true, active, label));
    let hot = tab.hovered() || x.hovered() || ui.rect_contains_pointer(rect);
    let visuals = ui.visuals();
    let accent = visuals.hyperlink_color;
    let weak = visuals.weak_text_color();
    if active {
        ui.painter().rect_filled(rect, 0.0, visuals.panel_fill);
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 1.0,
            egui::Stroke::new(2.0, accent),
        );
    } else if hot {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.bg_fill);
    }
    let color = if active { strong } else { weak };
    ui.painter().galley(
        egui::pos2(rect.left() + pad, rect.center().y - galley.size().y * 0.5),
        ui.painter()
            .layout_no_wrap(label.to_owned(), font.clone(), color),
        color,
    );
    if closable && (active || hot) {
        if x.hovered() {
            ui.painter().rect_filled(
                x_rect.shrink(side * 0.2),
                ui.visuals().widgets.hovered.corner_radius,
                ui.visuals().widgets.active.bg_fill,
            );
        }
        let c = x_rect.center();
        let r = side * 0.14;
        let stroke = egui::Stroke::new(1.3, if x.hovered() { strong } else { weak });
        ui.painter()
            .line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
        ui.painter()
            .line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
    }
    (tab, x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tabs(names: &[&str]) -> Dock {
        Dock::Tabs {
            tabs: names.iter().map(|s| (*s).to_owned()).collect(),
            active: 0,
        }
    }

    fn sample() -> DockLayout {
        let root = split_beside(tabs(&["tree", "layers"]), tabs(&["props"]), Side::Right);
        DockLayout::new(root, Vec::new()).unwrap()
    }

    #[test]
    fn operations_keep_the_layout_valid() {
        let mut l = sample();
        l.move_panel(
            "props",
            &Target::Tab {
                anchor: "tree".into(),
                index: Some(0),
            },
        )
        .unwrap();
        assert_eq!(
            l.root(),
            &Dock::Tabs {
                tabs: vec!["props".into(), "tree".into(), "layers".into()],
                active: 0
            }
        );
        l.move_panel(
            "layers",
            &Target::Split {
                anchor: "layers".into(),
                side: Side::Bottom,
            },
        )
        .unwrap();
        assert_eq!(l.panels(), ["props", "tree", "layers"]);
        assert!(matches!(
            l.root(),
            Dock::Split {
                axis: Axis::Vertical,
                ..
            }
        ));
        l.float("tree", [10.0, 10.0, 200.0, 100.0]).unwrap();
        assert_eq!(l.floating().len(), 1);
        l.unfloat("tree").unwrap();
        assert!(l.floating().is_empty());
        l.activate("tree").unwrap();
        l.set_fraction(&[], 2.0);
        if let Dock::Split { fraction, .. } = l.root() {
            assert!((fraction - (1.0 - MIN_FRACTION)).abs() < 1e-6);
        }
        for p in ["props", "tree", "layers"] {
            l.close(p).unwrap();
        }
        assert_eq!(l, DockLayout::default());
        l.add("props", &Target::Root { side: Side::Left }).unwrap();
        assert_eq!(l.panels(), ["props"]);
    }

    #[test]
    fn failed_operations_change_nothing() {
        let mut l = sample();
        let before = l.clone();
        assert_eq!(
            l.add("tree", &Target::Root { side: Side::Left })
                .unwrap_err()
                .code(),
            "ui.dock.panel_duplicate"
        );
        assert_eq!(l.close("nope").unwrap_err().code(), "ui.dock.panel_unknown");
        assert_eq!(
            l.add(
                "x",
                &Target::Split {
                    anchor: "nope".into(),
                    side: Side::Left
                }
            )
            .unwrap_err()
            .code(),
            "ui.dock.panel_unknown"
        );
        assert_eq!(
            l.add("bad id", &Target::Root { side: Side::Left })
                .unwrap_err()
                .code(),
            "ui.dock.invalid"
        );
        assert_eq!(
            l.float("tree", [0.0, 0.0, f32::NAN, 1.0])
                .unwrap_err()
                .code(),
            "ui.dock.invalid"
        );
        // Splitting deeper than the limit fails.
        for i in 0..MAX_DEPTH {
            let _ = l.add(
                &format!("p{i}"),
                &Target::Split {
                    anchor: "props".into(),
                    side: Side::Left,
                },
            );
        }
        let deep = l.clone();
        assert_eq!(
            l.add(
                "last",
                &Target::Split {
                    anchor: "props".into(),
                    side: Side::Left
                }
            )
            .unwrap_err()
            .code(),
            "ui.dock.too_deep"
        );
        assert_eq!(l, deep);
        assert_ne!(l, before);
    }

    #[test]
    fn hostile_node_graphs_are_rejected() {
        let bad = [
            r#"{"root":0,"nodes":[{"kind":"split","axis":"horizontal","fraction":0.5,"first":0,"second":1},{"kind":"tabs","tabs":["a"]}]}"#,
            r#"{"root":0,"nodes":[{"kind":"split","axis":"horizontal","fraction":0.5,"first":1,"second":1},{"kind":"tabs","tabs":["a"]}]}"#,
            r#"{"root":0,"nodes":[{"kind":"tabs","tabs":["a"]},{"kind":"tabs","tabs":["b"]}]}"#,
            r#"{"root":5,"nodes":[]}"#,
            r#"{"root":0,"nodes":[{"kind":"tabs","tabs":["a","a"]}]}"#,
            r#"{"root":0,"nodes":[{"kind":"tabs","tabs":["a"],"active":1}]}"#,
            r#"{"root":0,"nodes":[{"kind":"tabs","tabs":["a"]}],"extra":1}"#,
            r#"{"root":0,"nodes":[{"kind":"tabs","tabs":[]}],"floating":[{"root":0,"rect":[0,0,1,1]}]}"#,
        ];
        for json in bad {
            assert!(serde_json::from_str::<DockLayout>(json).is_err(), "{json}");
        }
        // A chain deeper than the limit.
        let mut nodes = Vec::new();
        for i in 0..=MAX_DEPTH {
            nodes.push(format!(
                r#"{{"kind":"split","axis":"vertical","fraction":0.5,"first":{},"second":{}}}"#,
                2 * i + 1,
                2 * i + 2
            ));
            nodes.push(format!(r#"{{"kind":"tabs","tabs":["p{i}"]}}"#));
        }
        nodes.push(r#"{"kind":"tabs","tabs":["end"]}"#.into());
        // Node 2i is a split whose first child 2i+1 is a group and second 2i+2 the next split.
        let json = format!(r#"{{"root":0,"nodes":[{}]}}"#, nodes.join(","));
        let error = serde_json::from_str::<DockLayout>(&json)
            .unwrap_err()
            .to_string();
        assert!(error.contains("ui.dock.too_deep"), "{error}");
    }

    fn arb_dock(depth: u32) -> impl Strategy<Value = Dock> {
        let leaf = proptest::collection::vec("[a-z][a-z0-9]{0,6}", 1..4)
            .prop_map(|tabs| Dock::Tabs { tabs, active: 0 });
        leaf.prop_recursive(depth, 64, 2, |inner| {
            (
                inner.clone(),
                inner,
                prop_oneof![Just(Axis::Horizontal), Just(Axis::Vertical)],
                0.05f32..0.95,
            )
                .prop_map(|(a, b, axis, fraction)| Dock::Split {
                    axis,
                    fraction,
                    first: Box::new(a),
                    second: Box::new(b),
                })
        })
    }

    #[derive(Debug, Clone, Serialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum HostileNode {
        Tabs {
            tabs: Vec<String>,
            active: usize,
        },
        Split {
            axis: Axis,
            fraction: f32,
            first: usize,
            second: usize,
        },
    }

    proptest! {
        #[test]
        fn layouts_round_trip(root in arb_dock(6), floats in proptest::collection::vec(arb_dock(2), 0..3)) {
            let floating = floats.into_iter().map(|dock| Floating { dock, rect: [1.0, 2.0, 300.0, 200.0] }).collect();
            if let Ok(layout) = DockLayout::new(root, floating) {
                let json = serde_json::to_string(&layout).unwrap();
                let back: DockLayout = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(back, layout);
            }
        }

        #[test]
        fn hostile_layouts_are_rejected_within_limits(
            nodes in proptest::collection::vec(
                prop_oneof![
                    (proptest::collection::vec("[a-c ]{0,3}", 0..4), 0usize..5)
                        .prop_map(|(tabs, active)| HostileNode::Tabs { tabs, active }),
                    (prop_oneof![Just(Axis::Horizontal), Just(Axis::Vertical)],
                     prop_oneof![Just(f32::NAN), -1.0f32..2.0, Just(f32::MAX)], 0usize..40, 0usize..40)
                        .prop_map(|(axis, fraction, first, second)| HostileNode::Split { axis, fraction, first, second }),
                ],
                0..40,
            ),
            root in 0usize..40,
        ) {
            let json = serde_json::json!({ "root": root, "nodes": nodes });
            if let Ok(layout) = serde_json::from_value::<DockLayout>(json) {
                prop_assert!(layout.check().is_ok());
                prop_assert!(layout.panels().len() <= MAX_PANELS);
            }
        }

        #[test]
        fn random_operations_keep_invariants(ops in proptest::collection::vec((0u8..6, 0usize..6, 0usize..6, 0u8..4), 0..40)) {
            let names = ["a", "b", "c", "d", "e", "f"];
            let sides = [Side::Left, Side::Right, Side::Top, Side::Bottom];
            let mut l = DockLayout::default();
            for (op, p, q, s) in ops {
                let (p, q, side) = (names[p], names[q], sides[usize::from(s)]);
                let _ = match op {
                    0 => l.add(p, &Target::Split { anchor: q.into(), side }),
                    1 => l.add(p, &Target::Root { side }),
                    2 => l.move_panel(p, &Target::Tab { anchor: q.into(), index: Some(1) }),
                    3 => l.close(p),
                    4 => l.float(p, [0.0, 0.0, 10.0, 10.0]),
                    _ => l.unfloat(p),
                };
                prop_assert!(l.check().is_ok());
                let back: DockLayout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
                prop_assert_eq!(&back, &l);
            }
        }
    }
}
