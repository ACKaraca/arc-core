//! The tree and outliner (CORE-UI-012).
//!
//! A [`TreeView`] shows a hierarchy that a [`TreeSource`] supplies lazily: child counts and
//! single children by index, filtered in the source. Only expanded nodes are asked for their
//! children, and only the rows inside the scrolled window are fetched, at most
//! [`MAX_ROWS_PER_FRAME`] per frame, so the cost of a frame does not grow with the size of the
//! hierarchy. The view remembers each expanded node with its parent and index and checks those
//! against the source every frame.
//!
//! Selection, rename, visibility, lock and drag and drop are [`Intent`]s for the commands of a
//! [`TreeCommands`]; expansion, the filter text and the rename text are view state.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use arc_base::ObjectId;
use egui::{Button, Key, Modifiers, Role, ScrollArea, Sense, TextEdit, Ui, WidgetInfo};

use crate::command::{Intent, object_list};
use crate::text::{Localize, messages};

/// The most rows fetched and drawn per frame.
pub const MAX_ROWS_PER_FRAME: usize = 256;
/// The most expanded nodes remembered.
pub const MAX_EXPANDED: usize = 4096;
/// The deepest level shown.
pub const MAX_DEPTH: usize = 64;
/// The most rows one Shift range selection covers.
pub const MAX_RANGE: usize = 10_000;
/// Filter characters passed to the source.
pub const MAX_FILTER_CHARS: usize = 256;

/// One node as the source describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeItem {
    /// The object.
    pub id: ObjectId,
    /// The shown name (user data, not a message).
    pub label: String,
    /// The node may have children.
    pub has_children: bool,
    /// The node is selected.
    pub selected: bool,
    /// The visibility toggle state; `None` hides the toggle.
    pub visible: Option<bool>,
    /// The lock toggle state; `None` hides the toggle.
    pub locked: Option<bool>,
}

/// A lazily loaded, filtered hierarchy. `parent` is `None` for the top level. Implementations
/// answer from indexes kept in the source; the view calls them only for expanded nodes and
/// visible rows.
pub trait TreeSource {
    /// The number of children of `parent` that match `filter`.
    fn child_count(&self, parent: Option<ObjectId>, filter: &str) -> usize;
    /// The child of `parent` at `index` among those matching `filter`.
    fn child(&self, parent: Option<ObjectId>, index: usize, filter: &str) -> Option<TreeItem>;
    /// The index of `id` among the children of `parent` matching `filter`.
    fn index_of(&self, parent: Option<ObjectId>, id: ObjectId, filter: &str) -> Option<usize>;
}

/// The commands tree intents name, with their parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeCommands {
    /// `objects` and `mode` (`replace`, `toggle`).
    pub select: String,
    /// `object` and `name`.
    pub rename: String,
    /// `objects` and `visible`.
    pub set_visible: String,
    /// `objects` and `locked`.
    pub set_locked: String,
    /// `objects` and `parent`.
    pub move_to: String,
}

/// A shown row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The node.
    pub item: TreeItem,
    /// The level, 0 at the top.
    pub depth: usize,
    /// The parent.
    pub parent: Option<ObjectId>,
    /// The index among the parent's children.
    pub index: usize,
}

/// The tree view's state.
#[derive(Debug, Clone, Default)]
pub struct TreeView {
    /// The filter text the source applies.
    pub filter: String,
    /// Expanded nodes with their parent and index.
    expanded: BTreeMap<ObjectId, (Option<ObjectId>, usize)>,
    renaming: Option<(ObjectId, String)>,
    anchor: Option<usize>,
}

#[derive(Clone, Copy)]
struct Walk<'a> {
    source: &'a dyn TreeSource,
    filter: &'a str,
    children: &'a HashMap<Option<ObjectId>, Vec<(usize, ObjectId)>>,
    sizes: &'a HashMap<ObjectId, usize>,
}

impl TreeView {
    fn filter_text(&self) -> String {
        self.filter.chars().take(MAX_FILTER_CHARS).collect()
    }

    /// True when `id` is expanded.
    #[must_use]
    pub fn is_expanded(&self, id: ObjectId) -> bool {
        self.expanded.contains_key(&id)
    }

    /// Expands the node at `index` of `parent`; beyond [`MAX_EXPANDED`] nodes it stays
    /// collapsed.
    pub fn expand(&mut self, id: ObjectId, parent: Option<ObjectId>, index: usize) {
        if self.expanded.len() < MAX_EXPANDED || self.expanded.contains_key(&id) {
            self.expanded.insert(id, (parent, index));
        }
    }

    /// Collapses `id`.
    pub fn collapse(&mut self, id: ObjectId) {
        self.expanded.remove(&id);
    }

    /// Checks every expanded node against the source: a node found at another index is moved
    /// there, a node no longer under its parent is forgotten.
    pub fn refresh(&mut self, source: &dyn TreeSource) {
        let filter = self.filter_text();
        self.expanded.retain(|id, (parent, index)| {
            if source
                .child(*parent, *index, &filter)
                .is_some_and(|c| c.id == *id)
            {
                return true;
            }
            match source.index_of(*parent, *id, &filter) {
                Some(found) => {
                    *index = found;
                    true
                }
                None => false,
            }
        });
    }

    fn children(&self) -> HashMap<Option<ObjectId>, Vec<(usize, ObjectId)>> {
        let mut out: HashMap<Option<ObjectId>, Vec<(usize, ObjectId)>> = HashMap::new();
        for (id, (parent, index)) in &self.expanded {
            out.entry(*parent).or_default().push((*index, *id));
        }
        for list in out.values_mut() {
            list.sort_unstable();
            list.dedup_by_key(|(index, _)| *index);
        }
        out
    }

    /// The rows below each expanded node (not counting the node), for nodes reachable from the
    /// top within [`MAX_DEPTH`] levels.
    fn sizes(
        source: &dyn TreeSource,
        filter: &str,
        children: &HashMap<Option<ObjectId>, Vec<(usize, ObjectId)>>,
    ) -> (usize, HashMap<ObjectId, usize>) {
        fn size(
            parent: Option<ObjectId>,
            depth: usize,
            source: &dyn TreeSource,
            filter: &str,
            children: &HashMap<Option<ObjectId>, Vec<(usize, ObjectId)>>,
            out: &mut HashMap<ObjectId, usize>,
        ) -> usize {
            let count = source.child_count(parent, filter);
            let mut total = count;
            if depth < MAX_DEPTH {
                for (index, id) in children.get(&parent).into_iter().flatten() {
                    if *index < count && !out.contains_key(id) {
                        out.insert(*id, 0);
                        let below = size(Some(*id), depth + 1, source, filter, children, out);
                        out.insert(*id, below);
                        total = total.saturating_add(below);
                    }
                }
            }
            total
        }
        let mut out = HashMap::new();
        let total = size(None, 0, source, filter, children, &mut out);
        (total, out)
    }

    /// The number of shown rows.
    #[must_use]
    pub fn row_count(&self, source: &dyn TreeSource) -> usize {
        let children = self.children();
        Self::sizes(source, &self.filter_text(), &children).0
    }

    /// The rows in `range`, at most [`MAX_ROWS_PER_FRAME`] of them.
    #[must_use]
    pub fn rows(&self, source: &dyn TreeSource, range: Range<usize>) -> Vec<Row> {
        let filter = self.filter_text();
        let children = self.children();
        let (_, sizes) = Self::sizes(source, &filter, &children);
        let walk = Walk {
            source,
            filter: &filter,
            children: &children,
            sizes: &sizes,
        };
        let want = range
            .end
            .saturating_sub(range.start)
            .min(MAX_ROWS_PER_FRAME);
        let mut out = Vec::with_capacity(want);
        let mut skip = range.start;
        Self::walk(walk, None, 0, &mut skip, want, &mut out);
        out
    }

    fn walk(
        walk: Walk<'_>,
        parent: Option<ObjectId>,
        depth: usize,
        skip: &mut usize,
        want: usize,
        out: &mut Vec<Row>,
    ) {
        let count = walk.source.child_count(parent, walk.filter);
        let expanded: &[(usize, ObjectId)] = walk.children.get(&parent).map_or(&[], Vec::as_slice);
        let mut next = expanded
            .iter()
            .filter(|(i, id)| *i < count && walk.sizes.contains_key(id))
            .peekable();
        let mut index = 0;
        while index < count && out.len() < want {
            // Rows up to and including the next expanded node are plain children.
            let block_end = next.peek().map_or(count, |(i, _)| i + 1);
            let block = block_end - index;
            if *skip >= block {
                *skip -= block;
                index = block_end;
            } else {
                index += *skip;
                *skip = 0;
                while index < block_end && out.len() < want {
                    if let Some(item) = walk.source.child(parent, index, walk.filter) {
                        out.push(Row {
                            item,
                            depth,
                            parent,
                            index,
                        });
                    }
                    index += 1;
                }
            }
            if index == block_end
                && let Some((at, id)) = next.next()
                && *at + 1 == block_end
            {
                let below = walk.sizes.get(id).copied().unwrap_or(0);
                if *skip >= below {
                    *skip -= below;
                } else {
                    Self::walk(walk, Some(*id), depth + 1, skip, want, out);
                }
            }
        }
    }

    /// Draws the filter field and the visible rows and returns the intent of a selection,
    /// rename, toggle or drop.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        source: &dyn TreeSource,
        commands: &TreeCommands,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let label = ui.label(messages::TREE_FILTER.text(localize));
        ui.text_edit_singleline(&mut self.filter)
            .labelled_by(label.id);
        self.refresh(source);
        let total = self.row_count(source);
        let height = ui.spacing().interact_size.y;
        let mut intent = None;
        ScrollArea::vertical().auto_shrink([false, true]).show_rows(
            ui,
            height,
            total,
            |ui, range| {
                let start = range.start;
                for (offset, row) in self.rows(source, range).into_iter().enumerate() {
                    if let Some(found) =
                        self.row(ui, source, &row, start + offset, commands, localize)
                    {
                        intent = Some(found);
                    }
                }
            },
        );
        intent
    }

    #[allow(clippy::too_many_lines, reason = "one row with its controls")]
    fn row(
        &mut self,
        ui: &mut Ui,
        source: &dyn TreeSource,
        row: &Row,
        at: usize,
        commands: &TreeCommands,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let id = row.item.id;
        let mut intent = None;
        ui.horizontal(|ui| {
            ui.add_space(
                ui.spacing().indent * f32::from(u8::try_from(row.depth.min(32)).unwrap_or(32)),
            );
            if row.item.has_children {
                let open = self.is_expanded(id);
                let name = if open {
                    messages::TREE_COLLAPSE
                } else {
                    messages::TREE_EXPAND
                }
                .text(localize);
                let toggle = ui.small_button(if open { "▾" } else { "▸" });
                toggle.widget_info(|| {
                    WidgetInfo::labeled(Role::Button, true, format!("{name} {}", row.item.label))
                });
                if toggle.clicked() {
                    if open {
                        self.collapse(id);
                    } else {
                        self.expand(id, row.parent, row.index);
                    }
                }
            }
            if let Some(visible) = row.item.visible {
                let name = messages::TREE_VISIBLE.text(localize);
                let mut on = visible;
                let r = ui.checkbox(&mut on, "");
                r.widget_info(|| {
                    WidgetInfo::selected(
                        Role::CheckBox,
                        true,
                        visible,
                        format!("{name} {}", row.item.label),
                    )
                });
                if r.clicked() {
                    intent = Some(
                        Intent::new(commands.set_visible.clone())
                            .with("objects", object_list(&[id]))
                            .with("visible", !visible),
                    );
                }
            }
            if let Some(locked) = row.item.locked {
                let name = messages::TREE_LOCKED.text(localize);
                let mut on = locked;
                let r = ui.checkbox(&mut on, "");
                r.widget_info(|| {
                    WidgetInfo::selected(
                        Role::CheckBox,
                        true,
                        locked,
                        format!("{name} {}", row.item.label),
                    )
                });
                if r.clicked() {
                    intent = Some(
                        Intent::new(commands.set_locked.clone())
                            .with("objects", object_list(&[id]))
                            .with("locked", !locked),
                    );
                }
            }
            if let Some((renamed, text)) = &mut self.renaming
                && *renamed == id
            {
                let edit_id = ui.make_persistent_id(("arc-tree-rename", id));
                let edit = ui.add(TextEdit::singleline(text).id(edit_id));
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if edit.lost_focus() {
                    let escaped = ui.input(|i| i.key_pressed(Key::Escape));
                    if !escaped && !text.trim().is_empty() && text.as_str() != row.item.label {
                        intent = Some(
                            Intent::new(commands.rename.clone())
                                .with("object", id.to_string())
                                .with("name", text.trim()),
                        );
                    }
                    self.renaming = None;
                }
                return;
            }
            let response = ui.add(
                Button::selectable(row.item.selected, row.item.label.as_str())
                    .sense(Sense::click_and_drag()),
            );
            if response.drag_started() {
                response.dnd_set_drag_payload(id);
            }
            if let Some(dragged) = response.dnd_release_payload::<ObjectId>()
                && *dragged != id
            {
                intent = Some(
                    Intent::new(commands.move_to.clone())
                        .with("objects", object_list(&[*dragged]))
                        .with("parent", id.to_string()),
                );
            }
            let mut keys = (false, false, false, false);
            if response.has_focus() {
                keys = ui.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::ArrowRight),
                        i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                        i.consume_key(Modifiers::NONE, Key::F2),
                        i.consume_key(Modifiers::NONE, Key::Space),
                    )
                });
            }
            let (right, left, rename, space) = keys;
            if right && row.item.has_children {
                self.expand(id, row.parent, row.index);
            }
            if left {
                self.collapse(id);
            }
            if rename || response.double_clicked() {
                self.renaming = Some((id, row.item.label.clone()));
                return;
            }
            if response.clicked() || space {
                let modifiers = ui.input(|i| i.modifiers);
                intent = Some(self.select(source, at, id, modifiers, commands));
            }
        });
        intent
    }

    fn select(
        &mut self,
        source: &dyn TreeSource,
        at: usize,
        id: ObjectId,
        modifiers: Modifiers,
        commands: &TreeCommands,
    ) -> Intent {
        let select = Intent::new(commands.select.clone());
        if modifiers.shift
            && let Some(anchor) = self.anchor
        {
            let (lo, hi) = (anchor.min(at), anchor.max(at));
            let hi = hi.min(lo + MAX_RANGE - 1);
            let mut ids = Vec::new();
            let mut start = lo;
            while start <= hi {
                let rows = self.rows(source, start..hi + 1);
                if rows.is_empty() {
                    break;
                }
                start += rows.len();
                ids.extend(rows.into_iter().map(|r| r.item.id));
            }
            return select
                .with("objects", object_list(&ids))
                .with("mode", "replace");
        }
        self.anchor = Some(at);
        let mode = if modifiers.command {
            "toggle"
        } else {
            "replace"
        };
        select
            .with("objects", object_list(&[id]))
            .with("mode", mode)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arc_base::id::Identifier;
    use proptest::prelude::*;
    use std::cell::Cell;

    /// A tree where node `n` has the children listed in `kids[n]`; the top level is `top`.
    #[derive(Default)]
    pub struct Fixed {
        pub top: Vec<u128>,
        pub kids: HashMap<u128, Vec<u128>>,
        pub calls: Cell<usize>,
    }

    impl Fixed {
        fn list(&self, parent: Option<ObjectId>) -> &[u128] {
            match parent {
                None => &self.top,
                Some(p) => self.kids.get(&p.as_u128()).map_or(&[], Vec::as_slice),
            }
        }
    }

    impl TreeSource for Fixed {
        fn child_count(&self, parent: Option<ObjectId>, _: &str) -> usize {
            self.list(parent).len()
        }
        fn child(&self, parent: Option<ObjectId>, index: usize, _: &str) -> Option<TreeItem> {
            self.calls.set(self.calls.get() + 1);
            let raw = *self.list(parent).get(index)?;
            Some(TreeItem {
                id: ObjectId::from_u128(raw),
                label: format!("n{raw}"),
                has_children: self.kids.contains_key(&raw),
                selected: false,
                visible: Some(true),
                locked: Some(false),
            })
        }
        fn index_of(&self, parent: Option<ObjectId>, id: ObjectId, _: &str) -> Option<usize> {
            self.list(parent).iter().position(|n| *n == id.as_u128())
        }
    }

    /// One million top-level nodes, each with three children.
    pub struct Huge {
        pub calls: Cell<usize>,
    }

    impl TreeSource for Huge {
        fn child_count(&self, parent: Option<ObjectId>, _: &str) -> usize {
            if parent.is_none() { 1_000_000 } else { 3 }
        }
        fn child(&self, parent: Option<ObjectId>, index: usize, _: &str) -> Option<TreeItem> {
            self.calls.set(self.calls.get() + 1);
            let base = parent.map_or(1, |p| p.as_u128() * 10);
            Some(TreeItem {
                id: ObjectId::from_u128(base + index as u128),
                label: format!("n{index}"),
                has_children: parent.is_none(),
                selected: false,
                visible: None,
                locked: None,
            })
        }
        fn index_of(&self, _: Option<ObjectId>, id: ObjectId, _: &str) -> Option<usize> {
            usize::try_from(id.as_u128().checked_sub(1)?).ok()
        }
    }

    fn naive(source: &Fixed, view: &TreeView) -> Vec<(u128, usize)> {
        fn go(
            source: &Fixed,
            view: &TreeView,
            parent: Option<ObjectId>,
            depth: usize,
            out: &mut Vec<(u128, usize)>,
        ) {
            for (i, raw) in source.list(parent).iter().enumerate() {
                out.push((*raw, depth));
                let id = ObjectId::from_u128(*raw);
                if view.expanded.get(&id) == Some(&(parent, i)) && depth < MAX_DEPTH {
                    go(source, view, Some(id), depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        go(source, view, None, 0, &mut out);
        out
    }

    #[test]
    fn rows_follow_expansion_and_windows() {
        let mut s = Fixed {
            top: vec![1, 2, 3],
            ..Fixed::default()
        };
        s.kids.insert(2, vec![20, 21]);
        s.kids.insert(21, vec![210]);
        let mut v = TreeView::default();
        v.expand(ObjectId::from_u128(2), None, 1);
        v.expand(ObjectId::from_u128(21), Some(ObjectId::from_u128(2)), 1);
        let ids: Vec<(u128, usize)> = v
            .rows(&s, 0..100)
            .iter()
            .map(|r| (r.item.id.as_u128(), r.depth))
            .collect();
        assert_eq!(ids, [(1, 0), (2, 0), (20, 1), (21, 1), (210, 2), (3, 0)]);
        assert_eq!(v.row_count(&s), 6);
        let window: Vec<u128> = v
            .rows(&s, 3..5)
            .iter()
            .map(|r| r.item.id.as_u128())
            .collect();
        assert_eq!(window, [21, 210]);
        // The source moved node 2; refresh follows it, and forgets a node that is gone.
        s.top = vec![2, 1];
        s.kids.remove(&2);
        s.kids.insert(2, vec![21]);
        v.refresh(&s);
        assert_eq!(v.expanded.get(&ObjectId::from_u128(2)), Some(&(None, 0)));
        assert_eq!(
            v.expanded.get(&ObjectId::from_u128(21)),
            Some(&(Some(ObjectId::from_u128(2)), 0))
        );
    }

    #[test]
    fn a_frame_fetches_a_bounded_number_of_rows() {
        let s = Huge {
            calls: Cell::new(0),
        };
        let mut v = TreeView::default();
        v.expand(ObjectId::from_u128(500_001), None, 500_000);
        assert_eq!(v.row_count(&s), 1_000_003);
        s.calls.set(0);
        let rows = v.rows(&s, 499_999..1_000_000);
        assert_eq!(rows.len(), MAX_ROWS_PER_FRAME);
        assert_eq!(rows[2].depth, 1);
        assert!(s.calls.get() <= MAX_ROWS_PER_FRAME);
    }

    proptest! {
        #[test]
        fn rows_match_a_full_walk(
            shape in proptest::collection::vec(0usize..4, 1..30),
            expand in proptest::collection::vec(any::<bool>(), 30),
            start in 0usize..40, len in 0usize..40,
        ) {
            // Node k (1-based) has shape[k-1] children numbered after it.
            let mut s = Fixed::default();
            let mut next = 1u128;
            let mut queue = vec![None::<u128>];
            let mut view = TreeView::default();
            let mut k = 0;
            while let Some(parent) = queue.pop() {
                let n = if parent.is_none() { 3 } else { shape.get(k).copied().unwrap_or(0) };
                k += 1;
                let mut list = Vec::new();
                for i in 0..n {
                    if next > 60 { break; }
                    list.push(next);
                    if expand.get(next as usize % 30).copied().unwrap_or(false) {
                        view.expand(ObjectId::from_u128(next), parent.map(ObjectId::from_u128), i);
                    }
                    queue.push(Some(next));
                    next += 1;
                }
                match parent {
                    None => s.top = list,
                    Some(p) => { if !list.is_empty() { s.kids.insert(p, list); } }
                }
            }
            let full = naive(&s, &view);
            prop_assert_eq!(view.row_count(&s), full.len());
            let got: Vec<(u128, usize)> = view.rows(&s, start..start + len).iter().map(|r| (r.item.id.as_u128(), r.depth)).collect();
            let want: Vec<(u128, usize)> = full.iter().skip(start).take(len).copied().collect();
            prop_assert_eq!(got, want);
        }
    }
}
