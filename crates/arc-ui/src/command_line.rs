//! The command line widget (CORE-UI-006) with the Escape and Enter standard (CORE-UI-031) and
//! undo keys (CORE-UI-032).
//!
//! The widget draws the state of an `arc_command::CommandLine` driver: the active prompt with its
//! keyword options, the input field, the completion candidates the caller computed with
//! `CommandLine::complete`, and a searchable history. It never calls the driver: it returns a
//! [`LineAction`] that the application applies (`submit`, `repeat_last`, `back`, `cancel`, or
//! invoking an [`Intent`]). [`escape`], [`enter`], [`undo`] and [`redo`] are the decisions as
//! pure functions of a [`LineStatus`].

use std::collections::VecDeque;

use arc_base::error::Value;
use arc_command::line::{Completion, MAX_LINE_BYTES};
use arc_command::{CommandLine, Prompt};
use egui::{Button, EventFilter, Key, Modifiers, TextEdit, Ui};

use crate::command::Intent;
use crate::text::{Direction, Localize, messages, resolve};

/// History entries kept.
pub const MAX_HISTORY: usize = 200;
/// Completion candidates shown.
pub const MAX_SHOWN_CANDIDATES: usize = 10;

/// The commands the line emits for keys that do not belong to the active command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCommands {
    /// Undo in the document.
    pub undo: String,
    /// Redo in the document.
    pub redo: String,
    /// Clear the selection.
    pub clear_selection: String,
}

/// What the key decisions read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineStatus {
    /// A command is active (prompting).
    pub active: bool,
    /// The answers the active command has collected.
    pub answers: usize,
    /// The selection is not empty.
    pub has_selection: bool,
    /// A completed command can be repeated.
    pub has_last: bool,
}

impl LineStatus {
    /// The status of `line` with the caller's selection state.
    #[must_use]
    pub fn of(line: &CommandLine, has_selection: bool) -> Self {
        Self {
            active: line.active().is_some(),
            answers: line.active().map_or(0, |s| s.answers().len()),
            has_selection,
            has_last: line.last().is_some(),
        }
    }
}

/// What the application does after a key or click in the command line.
#[derive(Debug, Clone, PartialEq)]
pub enum LineAction {
    /// Clear the typed text (the widget already did).
    ClearInput,
    /// `CommandLine::submit` with the text; an empty text accepts the prompt's default.
    Submit(String),
    /// `CommandLine::repeat_last`.
    Repeat,
    /// `CommandLine::back`: step back one answer.
    Back,
    /// `CommandLine::cancel`.
    Cancel,
    /// Invoke the intent.
    Invoke(Intent),
    /// `CommandLine::cancel`, then invoke the intent.
    CancelThen(Intent),
}

/// Escape: clear typed text, else step back one answer, else cancel the command at its first
/// prompt, else clear the selection; `None` when there is nothing to step back from.
#[must_use]
pub fn escape(status: LineStatus, typed: &str, commands: &LineCommands) -> Option<LineAction> {
    Some(if !typed.is_empty() {
        LineAction::ClearInput
    } else if status.active && status.answers > 0 {
        LineAction::Back
    } else if status.active {
        LineAction::Cancel
    } else if status.has_selection {
        LineAction::Invoke(Intent::new(commands.clear_selection.clone()))
    } else {
        return None;
    })
}

/// Enter: answer the prompt (empty text accepts its default), run a typed line, else repeat the
/// last command; `None` when idle with nothing to repeat.
#[must_use]
pub fn enter(status: LineStatus, typed: &str) -> Option<LineAction> {
    let typed = typed.trim();
    if status.active || !typed.is_empty() {
        Some(LineAction::Submit(typed.to_owned()))
    } else if status.has_last {
        Some(LineAction::Repeat)
    } else {
        None
    }
}

/// Undo: step back inside the active command first (cancelling it at its first prompt), else
/// undo in the document. It always resolves.
#[must_use]
pub fn undo(status: LineStatus, commands: &LineCommands) -> LineAction {
    if status.active && status.answers > 0 {
        LineAction::Back
    } else if status.active {
        LineAction::Cancel
    } else {
        LineAction::Invoke(Intent::new(commands.undo.clone()))
    }
}

/// Redo: redo in the document; an active command is cancelled first, because its answers may
/// refer to the state that redo replaces. It always resolves.
#[must_use]
pub fn redo(status: LineStatus, commands: &LineCommands) -> LineAction {
    let intent = Intent::new(commands.redo.clone());
    if status.active {
        LineAction::CancelThen(intent)
    } else {
        LineAction::Invoke(intent)
    }
}

/// Submitted lines, most recent first, without repeats.
#[derive(Debug, Clone, Default)]
pub struct History {
    entries: VecDeque<String>,
}

impl History {
    /// Records `line` (trimmed, cut to the driver's line limit); a blank line is not recorded.
    /// An earlier equal entry moves to the front; the oldest entry beyond [`MAX_HISTORY`] goes.
    pub fn push(&mut self, line: &str) {
        let mut line = line.trim();
        if line.is_empty() {
            return;
        }
        while line.len() > MAX_LINE_BYTES {
            let mut end = MAX_LINE_BYTES.min(line.len());
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line = line.get(..end).unwrap_or_default();
        }
        self.entries.retain(|e| e != line);
        self.entries.push_front(line.to_owned());
        self.entries.truncate(MAX_HISTORY);
    }

    /// The entries containing `query` (ignoring case), most recent first.
    pub fn search<'a>(&'a self, query: &str) -> impl Iterator<Item = &'a str> + 'a {
        let query = query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(move |e| e.to_lowercase().contains(&query))
            .map(String::as_str)
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// What the widget reads besides the driver.
#[derive(Clone, Copy)]
pub struct LineContext<'a> {
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The selection is not empty.
    pub has_selection: bool,
    /// The commands for undo, redo and clearing the selection.
    pub commands: &'a LineCommands,
}

/// The command line widget's state.
#[derive(Debug, Clone, Default)]
pub struct CommandLineView {
    /// The typed text.
    pub input: String,
    /// The submitted lines.
    pub history: History,
    /// The language tag of the labels; its direction orders the options and aligns the text.
    pub language: String,
    /// Overrides the direction the language gives.
    pub direction: Option<Direction>,
    highlighted: Option<usize>,
    recall: Option<(String, usize)>,
}

pub(crate) fn prompt_text(prompt: &Prompt, localize: &dyn Localize) -> String {
    let args: Vec<(&str, Value)> = prompt
        .message
        .args
        .iter()
        .map(|(k, v)| (k.as_str(), Value::Text(v.clone())))
        .collect();
    resolve(localize, &prompt.message.id, &args)
}

impl CommandLineView {
    /// Draws the prompt, its keyword options, the input field and the completion candidates of
    /// `completion` (computed by the caller for [`CommandLineView::input`]), and handles keys:
    /// Escape, Enter, Up and Down (completion highlight, or history entries containing the typed
    /// text), and Ctrl+Z, Ctrl+Y and Ctrl+Shift+Z (Cmd on macOS) while the field or nothing has
    /// focus, before the field's own text undo can take them.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        line: &CommandLine,
        completion: &Completion,
        context: &LineContext<'_>,
    ) -> Option<LineAction> {
        let status = LineStatus::of(line, context.has_selection);
        let prompt = line.active().and_then(|s| s.prompt());
        self.show_with(ui, status, prompt, true, completion, context)
    }

    /// Draws the command line for a prompt and status the caller supplies, instead of a
    /// [`CommandLine`]: a product whose controller keeps the line. `prompt` labels the field and,
    /// with `keywords`, adds its keyword options as buttons; a product that shows the options in
    /// its own option bar passes `keywords: false`. Keys and actions are the same as
    /// [`CommandLineView::show`].
    pub fn show_with(
        &mut self,
        ui: &mut Ui,
        status: LineStatus,
        prompt: Option<&Prompt>,
        keywords: bool,
        completion: &Completion,
        context: &LineContext<'_>,
    ) -> Option<LineAction> {
        let id = ui.make_persistent_id("arc-command-line");
        let focused = ui.memory(|m| m.focused());
        let in_field = focused == Some(id);
        let ours = in_field || focused.is_none();
        let shown = completion.candidates.len().min(MAX_SHOWN_CANDIDATES);
        let mut action = None;
        if ours {
            let keys = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
                    i.consume_key(Modifiers::COMMAND, Key::Z),
                    i.consume_key(Modifiers::COMMAND, Key::Y),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    in_field && i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    in_field && i.consume_key(Modifiers::NONE, Key::ArrowDown),
                )
            });
            let (redo_shift, undo_key, redo_key, esc, ret, up, down) = keys;
            if redo_shift || redo_key {
                action = Some(redo(status, context.commands));
            } else if undo_key {
                action = Some(undo(status, context.commands));
            } else if esc {
                action = escape(status, &self.input, context.commands);
                if action == Some(LineAction::ClearInput) {
                    self.input.clear();
                }
            } else if ret {
                match self.highlighted.and_then(|h| completion.candidates.get(h)) {
                    Some(candidate) => self.complete(completion, &candidate.text),
                    None => action = self.submit(status),
                }
            } else if (up || down) && shown > 0 && !self.input.is_empty() {
                let last = shown - 1;
                self.highlighted = Some(match (self.highlighted, down) {
                    (None, true) => 0,
                    (None, false) => last,
                    (Some(h), true) => (h + 1).min(last),
                    (Some(h), false) => h.saturating_sub(1),
                });
            } else if up || down {
                self.recall(up);
            }
        }

        let label_text = prompt.map_or_else(
            || messages::COMMAND_LINE.text(context.localize),
            |p| prompt_text(p, context.localize),
        );
        let direction = self.layout_direction();
        let label = if direction.is_right_to_left() {
            direction.row(ui, false, |ui| ui.label(label_text)).inner
        } else {
            ui.label(label_text)
        };
        if let Some(prompt) = prompt.filter(|_| keywords) {
            direction.row(ui, true, |ui| {
                for option in prompt.keyword_options() {
                    let mut text = resolve(context.localize, &option.label, &[]);
                    if let Some(c) = option.shortcut {
                        text = format!("{text} ({c})");
                    }
                    if ui.button(text).clicked() {
                        action = Some(LineAction::Submit(option.id.clone()));
                    }
                }
            });
        }
        let field = ui
            .add(
                TextEdit::singleline(&mut self.input)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .align(direction.start_align())
                    // Escape and the vertical arrows belong to the command line, not to
                    // focus movement.
                    .event_filter(EventFilter {
                        tab: false,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    }),
            )
            .labelled_by(label.id);
        if field.changed() {
            self.highlighted = None;
            self.recall = None;
        }
        if !self.input.is_empty() {
            for (index, candidate) in completion.candidates.iter().take(shown).enumerate() {
                let button =
                    Button::selectable(self.highlighted == Some(index), candidate.text.as_str());
                if ui.add(button).clicked() {
                    self.complete(completion, &candidate.text);
                    field.request_focus();
                }
            }
        }
        action
    }

    /// The layout direction: [`CommandLineView::direction`] when set, else the direction of
    /// [`CommandLineView::language`].
    #[must_use]
    pub fn layout_direction(&self) -> Direction {
        Direction::resolve(&self.language, self.direction)
    }

    fn submit(&mut self, status: LineStatus) -> Option<LineAction> {
        let action = enter(status, &self.input);
        if matches!(action, Some(LineAction::Submit(_))) {
            self.history.push(&self.input);
            self.input.clear();
        }
        self.highlighted = None;
        self.recall = None;
        action
    }

    fn complete(&mut self, completion: &Completion, text: &str) {
        let span = completion
            .span
            .map_or(0..self.input.len(), |s| s.start..s.end);
        if self.input.get(span.clone()).is_some() {
            self.input.replace_range(span, text);
        }
        self.highlighted = None;
    }

    /// Up (`older`) or Down through the history entries containing the text typed before
    /// recall started.
    fn recall(&mut self, older: bool) {
        let (query, at) = self
            .recall
            .clone()
            .unwrap_or((self.input.clone(), usize::MAX));
        let found: Vec<&str> = self.history.search(&query).collect();
        let next = match (at, older) {
            (usize::MAX, true) => 0,
            (usize::MAX, false) => return,
            (n, true) => (n + 1).min(found.len().saturating_sub(1)),
            (0, false) => {
                self.input = query;
                self.recall = None;
                return;
            }
            (n, false) => n - 1,
        };
        if let Some(entry) = found.get(next) {
            self.input = (*entry).to_owned();
            self.recall = Some((query, next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> LineCommands {
        LineCommands {
            undo: "core.edit.undo".into(),
            redo: "core.edit.redo".into(),
            clear_selection: "core.select.clear".into(),
        }
    }

    fn status(active: bool, answers: usize, has_selection: bool, has_last: bool) -> LineStatus {
        LineStatus {
            active,
            answers,
            has_selection,
            has_last,
        }
    }

    #[test]
    fn escape_steps_back_through_text_prompt_command_and_selection() {
        let c = commands();
        assert_eq!(
            escape(status(true, 2, true, true), "12", &c),
            Some(LineAction::ClearInput)
        );
        assert_eq!(
            escape(status(true, 2, true, true), "", &c),
            Some(LineAction::Back)
        );
        assert_eq!(
            escape(status(true, 0, true, true), "", &c),
            Some(LineAction::Cancel)
        );
        assert_eq!(
            escape(status(false, 0, true, true), "", &c),
            Some(LineAction::Invoke(Intent::new("core.select.clear")))
        );
        assert_eq!(escape(status(false, 0, false, true), "", &c), None);
    }

    #[test]
    fn enter_accepts_default_runs_line_or_repeats() {
        assert_eq!(
            enter(status(true, 1, false, true), "  "),
            Some(LineAction::Submit(String::new()))
        );
        assert_eq!(
            enter(status(false, 0, false, true), " line 3 "),
            Some(LineAction::Submit("line 3".into()))
        );
        assert_eq!(
            enter(status(false, 0, false, true), ""),
            Some(LineAction::Repeat)
        );
        assert_eq!(enter(status(false, 0, false, false), ""), None);
    }

    #[test]
    fn undo_and_redo_always_resolve() {
        let c = commands();
        assert_eq!(undo(status(true, 1, false, false), &c), LineAction::Back);
        assert_eq!(undo(status(true, 0, false, false), &c), LineAction::Cancel);
        assert_eq!(
            undo(status(false, 0, false, false), &c),
            LineAction::Invoke(Intent::new("core.edit.undo"))
        );
        assert_eq!(
            redo(status(true, 3, false, false), &c),
            LineAction::CancelThen(Intent::new("core.edit.redo"))
        );
        assert_eq!(
            redo(status(false, 0, false, false), &c),
            LineAction::Invoke(Intent::new("core.edit.redo"))
        );
    }

    #[test]
    fn history_is_bounded_searchable_and_without_repeats() {
        let mut h = History::default();
        for i in 0..MAX_HISTORY + 20 {
            h.push(&format!("line {i}"));
        }
        h.push("  ");
        h.push("circle 5");
        h.push("line 300");
        h.push("circle 5");
        assert_eq!(h.len(), MAX_HISTORY);
        assert_eq!(h.search("CIRC").collect::<Vec<_>>(), ["circle 5"]);
        assert_eq!(h.search("").next(), Some("circle 5"));
        h.push(&"é".repeat(MAX_LINE_BYTES));
        assert!(
            h.search("é")
                .next()
                .is_some_and(|e| e.len() <= MAX_LINE_BYTES)
        );
    }

    #[test]
    fn recall_walks_matching_entries_and_returns_to_the_typed_text() {
        let mut v = CommandLineView::default();
        for line in ["move", "line 1", "copy", "line 2"] {
            v.history.push(line);
        }
        v.input = "line".into();
        v.recall(true);
        assert_eq!(v.input, "line 2");
        v.recall(true);
        assert_eq!(v.input, "line 1");
        v.recall(true);
        assert_eq!(v.input, "line 1");
        v.recall(false);
        v.recall(false);
        assert_eq!(v.input, "line");
    }
}
