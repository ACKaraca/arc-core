//! The command palette (CORE-UI-007).
//!
//! [`rank`] is a pure function from a query, the command views and the recent-use list to an
//! ordered list of matches. Only enabled commands are ranked. A command matches through its
//! localized label, its identifier, a user alias or a localized keyword; its match kind is the
//! best over these fields: exact, then prefix (of the field or of a word in it), then
//! subsequence (the query's characters in order). Results are ordered by match kind, then by
//! recent use, then by the compactness of a subsequence match, then by label and identifier.
//! Comparison is on lowercased text; the query is trimmed and cut to [`MAX_QUERY_CHARS`]
//! characters and each field to [`MAX_FIELD_CHARS`], so the work per command is bounded.

use std::cmp::Reverse;
use std::collections::VecDeque;

use egui::{Button, Key, Modifiers, Ui};

use crate::command::{CommandView, Intent};
use crate::text::{Localize, messages};

/// Query characters taken into account.
pub const MAX_QUERY_CHARS: usize = 64;
/// Field characters taken into account.
pub const MAX_FIELD_CHARS: usize = 256;
/// Aliases and keywords considered per command, each.
pub const MAX_TERMS: usize = 16;
/// Commands remembered as recently used.
pub const MAX_RECENT: usize = 50;
/// Results the widget shows.
pub const MAX_SHOWN: usize = 50;

/// How a command matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// The query's characters occur in order.
    Subsequence,
    /// A field or a word in it starts with the query; every command matches an empty query
    /// this way.
    Prefix,
    /// A field equals the query.
    Exact,
}

/// A ranked command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    /// The index into the commands passed to [`rank`].
    pub index: usize,
    /// How it matched.
    pub kind: MatchKind,
}

/// Recently invoked commands, most recent first.
#[derive(Debug, Clone, Default)]
pub struct Recent {
    ids: VecDeque<String>,
}

impl Recent {
    /// Moves `id` to the front, dropping the oldest entry beyond [`MAX_RECENT`].
    pub fn touch(&mut self, id: &str) {
        self.ids.retain(|known| known != id);
        self.ids.push_front(id.to_owned());
        self.ids.truncate(MAX_RECENT);
    }

    /// The position of `id`, 0 for the most recent.
    #[must_use]
    pub fn position(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|known| known == id)
    }
}

fn normalize(text: &str, max: usize) -> String {
    text.trim().to_lowercase().chars().take(max).collect()
}

/// The match kind of `query` in `field` and the span of a subsequence match.
fn match_field(query: &str, field: &str) -> Option<(MatchKind, usize)> {
    if field == query {
        return Some((MatchKind::Exact, 0));
    }
    let word_start = field
        .split(|c: char| c.is_whitespace() || matches!(c, '.' | '-' | '_'))
        .any(|word| word.starts_with(query));
    if field.starts_with(query) || word_start {
        return Some((MatchKind::Prefix, 0));
    }
    let mut wanted = query.chars().peekable();
    let (mut first, mut last) = (None, 0);
    for (at, c) in field.chars().enumerate() {
        if wanted.peek() == Some(&c) {
            wanted.next();
            first.get_or_insert(at);
            last = at;
        }
    }
    let first = first?;
    wanted
        .peek()
        .is_none()
        .then_some((MatchKind::Subsequence, last - first))
}

/// Match kind (best first), recency, subsequence span, label, identifier, index.
type SortKey<'a> = (Reverse<MatchKind>, usize, usize, &'a str, &'a str, usize);

/// Ranks the enabled `commands` for `query` and returns at most `limit` matches, best first.
#[must_use]
pub fn rank(query: &str, commands: &[CommandView], recent: &Recent, limit: usize) -> Vec<Ranked> {
    let query = normalize(query, MAX_QUERY_CHARS);
    let mut found: Vec<SortKey<'_>> = Vec::new();
    for (index, view) in commands.iter().enumerate() {
        if !view.is_enabled() {
            continue;
        }
        let best = if query.is_empty() {
            Some((MatchKind::Prefix, 0))
        } else {
            [view.label.as_str(), view.id.as_str()]
                .into_iter()
                .chain(view.aliases.iter().take(MAX_TERMS).map(String::as_str))
                .chain(view.keywords.iter().take(MAX_TERMS).map(String::as_str))
                .filter_map(|field| match_field(&query, &normalize(field, MAX_FIELD_CHARS)))
                .max_by_key(|&(kind, span)| (kind, Reverse(span)))
        };
        if let Some((kind, span)) = best {
            let recency = recent.position(&view.id).unwrap_or(usize::MAX);
            found.push((Reverse(kind), recency, span, &view.label, &view.id, index));
        }
    }
    found.sort_unstable();
    found
        .into_iter()
        .take(limit)
        .map(|(Reverse(kind), .., index)| Ranked { index, kind })
        .collect()
}

/// The palette widget's state.
#[derive(Debug, Clone, Default)]
pub struct Palette {
    /// The query text.
    pub query: String,
    selected: usize,
}

impl Palette {
    /// Draws the search field and the ranked commands with their bindings. Arrow keys move the
    /// highlight, Enter or a click returns the command's intent. The caller invokes it and
    /// records it in `recent`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        commands: &[CommandView],
        recent: &Recent,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let label = ui.label(messages::PALETTE_SEARCH.text(localize));
        let field = ui
            .text_edit_singleline(&mut self.query)
            .labelled_by(label.id);
        if field.changed() {
            self.selected = 0;
        }
        let ranked = rank(&self.query, commands, recent, MAX_SHOWN);
        if ranked.is_empty() {
            ui.label(messages::PALETTE_NO_RESULTS.text(localize));
            return None;
        }
        let (down, up, enter) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        let last = ranked.len() - 1;
        self.selected = if down {
            (self.selected + 1).min(last)
        } else if up {
            self.selected.saturating_sub(1)
        } else {
            self.selected.min(last)
        };
        let mut intent = None;
        for (row, ranked) in ranked.iter().enumerate() {
            let Some(view) = commands.get(ranked.index) else {
                continue;
            };
            let mut button = Button::selectable(row == self.selected, view.label.as_str());
            if let Some(binding) = &view.binding {
                button = button.shortcut_text(binding.as_str());
            }
            let clicked = ui.add(button).clicked();
            if clicked || (enter && row == self.selected) {
                intent = Some(Intent {
                    command: view.id.clone(),
                });
            }
        }
        intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn view(id: &str, label: &str) -> CommandView {
        CommandView {
            id: id.into(),
            label: label.into(),
            keywords: Vec::new(),
            aliases: Vec::new(),
            binding: None,
            disabled: None,
        }
    }

    fn ids(query: &str, commands: &[CommandView], recent: &Recent) -> Vec<String> {
        rank(query, commands, recent, usize::MAX)
            .iter()
            .map(|r| commands[r.index].id.clone())
            .collect()
    }

    #[test]
    fn fields_kinds_and_disabled_commands() {
        let mut keyworded = view("core.view.zoom_extents", "Zoom to Fit");
        keyworded.keywords = vec!["frame".into()];
        let mut aliased = view("core.file.save", "Save");
        aliased.aliases = vec!["sv".into()];
        let mut disabled = view("core.edit.undo", "Save Undo");
        disabled.disabled = Some("no".into());
        let all = vec![
            keyworded,
            aliased,
            disabled,
            view("core.file.save_as", "Save As"),
        ];
        let recent = Recent::default();
        assert_eq!(
            ids("save", &all, &recent),
            ["core.file.save", "core.file.save_as"]
        );
        assert_eq!(
            ids("SV", &all, &recent),
            ["core.file.save", "core.file.save_as"]
        );
        assert_eq!(ids("frame", &all, &recent), ["core.view.zoom_extents"]);
        assert_eq!(ids("zoom_ext", &all, &recent), ["core.view.zoom_extents"]);
        assert_eq!(ids("fit", &all, &recent), ["core.view.zoom_extents"]);
        assert_eq!(ids("ztf", &all, &recent), ["core.view.zoom_extents"]);
        assert!(ids("qqq", &all, &recent).is_empty());
        assert_eq!(rank("", &all, &recent, 2).len(), 2);
    }

    #[test]
    fn empty_query_lists_recent_first() {
        let all = vec![view("a.b.c", "Alpha"), view("a.b.d", "Beta")];
        let mut recent = Recent::default();
        recent.touch("a.b.d");
        assert_eq!(ids("", &all, &recent), ["a.b.d", "a.b.c"]);
        for i in 0..MAX_RECENT + 10 {
            recent.touch(&format!("x.y.{i}"));
        }
        assert_eq!(recent.position("a.b.d"), None);
    }

    fn label() -> impl Strategy<Value = String> {
        "[a-e ]{1,8}"
    }

    proptest! {
        #[test]
        fn exact_first_then_prefix_then_subsequence(
            query in "[a-e]{1,4}",
            labels in proptest::collection::vec(label(), 1..30),
            touched in proptest::collection::vec(0usize..30, 0..10),
        ) {
            let commands: Vec<CommandView> = labels.iter().enumerate()
                .map(|(i, l)| view(&format!("t.c.n{i}"), l)).collect();
            let mut recent = Recent::default();
            for t in touched { recent.touch(&format!("t.c.n{t}")); }
            let ranked = rank(&query, &commands, &recent, usize::MAX);
            for pair in ranked.windows(2) {
                prop_assert!(pair[0].kind >= pair[1].kind);
            }
            // An exact label is listed before every non-exact one.
            if let Some(first_other) = ranked.iter().position(|r| r.kind != MatchKind::Exact) {
                prop_assert!(ranked[first_other..].iter().all(|r| commands[r.index].label.trim() != query));
            }
        }

        #[test]
        fn recency_breaks_ties(label in label(), first in 0usize..5, second in 0usize..5) {
            prop_assume!(first != second);
            let commands: Vec<CommandView> = (0..5).map(|i| view(&format!("t.c.n{i}"), &label)).collect();
            let mut recent = Recent::default();
            recent.touch(&format!("t.c.n{second}"));
            recent.touch(&format!("t.c.n{first}"));
            let ranked = rank(&label, &commands, &recent, usize::MAX);
            prop_assert_eq!(ranked[0].index, first);
            prop_assert_eq!(ranked[1].index, second);
        }

        #[test]
        fn hostile_queries_never_panic_and_work_is_bounded(
            query in "\\PC{0,300}",
            labels in proptest::collection::vec("\\PC{0,40}", 0..20),
            limit in 0usize..10,
        ) {
            let commands: Vec<CommandView> = labels.iter().enumerate()
                .map(|(i, l)| view(&format!("t.c.n{i}"), l)).collect();
            let ranked = rank(&query, &commands, &Recent::default(), limit);
            prop_assert!(ranked.len() <= limit);
            // Only the first MAX_QUERY_CHARS characters count.
            let cut: String = query.trim().to_lowercase().chars().take(MAX_QUERY_CHARS).collect();
            let long = format!("{cut}{}", "z".repeat(10_000));
            if cut.chars().count() == MAX_QUERY_CHARS {
                prop_assert_eq!(rank(&long, &commands, &Recent::default(), limit), ranked);
            }
        }
    }
}
