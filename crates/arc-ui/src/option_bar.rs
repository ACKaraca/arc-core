//! The tool option bar (CORE-UI-008).
//!
//! The bar shows the active prompt of a command as controls: a button per keyword option and a
//! labeled field per parameter with its current value. It never calls a command driver. A
//! button returns the keyword identifier, and an edited field returns its text on Enter; the
//! application feeds the returned text to the command line exactly as typed input, so both paths
//! give the same answer.
//!
//! The application fills [`PromptControls`] from the `arc-command` prompt state: the keyword
//! options of the current [`arc_command::Prompt`], and for each parameter the preset value or the
//! latest answer (written as text), with `editable` set only for the parameter the current prompt
//! asks for. An empty [`PromptControls`] draws nothing.

use egui::{Button, Id, Key, Modifiers, Role, TextEdit, Ui, WidgetInfo};

use crate::text::{Direction, Localize, messages, resolve};

/// Keyword options shown; more are not drawn.
pub const MAX_OPTIONS: usize = 64;
/// Parameters shown; more are not drawn.
pub const MAX_PARAMETERS: usize = 64;

/// A keyword option of the active prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionControl {
    /// The identifier: the text the button returns, which the command line accepts as the option.
    pub id: String,
    /// The label's message identifier.
    pub label: String,
    /// The shortcut letter, shown in brackets after the label.
    pub shortcut: Option<char>,
}

/// A parameter of the active command with its current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterControl {
    /// The parameter name; it identifies the field across frames.
    pub name: String,
    /// The label's message identifier.
    pub label: String,
    /// The current value as the text the command line would take for it.
    pub value: String,
    /// Whether the field accepts input: true for the parameter the active prompt asks for.
    pub editable: bool,
}

/// The controls of the active prompt. The default is empty: no prompt is active.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromptControls {
    /// The keyword options of the active prompt, in prompt order.
    pub options: Vec<OptionControl>,
    /// The parameters of the command, in order.
    pub parameters: Vec<ParameterControl>,
}

impl PromptControls {
    /// True when there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.options.is_empty() && self.parameters.is_empty()
    }
}

/// An edit in progress: the field's text, and the value it started from.
#[derive(Debug, Clone)]
struct Draft {
    name: String,
    base: String,
    text: String,
}

/// The option bar's state: the language that sets the direction, and the edit in progress.
#[derive(Debug, Clone, Default)]
pub struct OptionBar {
    /// The language tag of the labels; its direction orders the controls.
    pub language: String,
    /// Overrides the direction the language gives.
    pub direction: Option<Direction>,
    draft: Option<Draft>,
}

impl OptionBar {
    /// The layout direction: [`OptionBar::direction`] when set, else the direction of
    /// [`OptionBar::language`].
    #[must_use]
    pub fn layout_direction(&self) -> Direction {
        Direction::resolve(&self.language, self.direction)
    }

    /// Draws `controls` in a horizontal bar, the first control at the logical start, and returns
    /// the text the command line would receive: a keyword identifier when a keyword button is
    /// clicked or activated, or the trimmed text of a parameter field when Enter is pressed in it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        controls: &PromptControls,
        localize: &dyn Localize,
    ) -> Option<String> {
        if controls.is_empty() {
            self.draft = None;
            return None;
        }
        if !controls.parameters.iter().any(|p| p.editable) {
            self.draft = None;
        }
        let direction = self.layout_direction();
        let mut chosen = None;
        let bar = direction.row(ui, true, |ui| {
            for option in controls.options.iter().take(MAX_OPTIONS) {
                let mut text = resolve(localize, &option.label, &[]);
                if let Some(c) = option.shortcut {
                    text = format!("{text} ({c})");
                }
                if ui.add(Button::new(text)).clicked() && chosen.is_none() {
                    chosen = Some(option.id.clone());
                }
            }
            for parameter in controls.parameters.iter().take(MAX_PARAMETERS) {
                let input = self.parameter(ui, parameter, localize);
                if chosen.is_none() {
                    chosen = input;
                }
            }
        });
        let label = messages::OPTION_BAR.text(localize);
        bar.response
            .widget_info(|| WidgetInfo::labeled(Role::Toolbar, true, label.as_str()));
        chosen
    }

    /// Draws one parameter: its label, then a field when it is editable or the value as text.
    /// Returns the trimmed text on Enter in the field.
    fn parameter(
        &mut self,
        ui: &mut Ui,
        parameter: &ParameterControl,
        localize: &dyn Localize,
    ) -> Option<String> {
        let name = resolve(localize, &parameter.label, &[]);
        let label = ui.label(name);
        if !parameter.editable {
            ui.label(parameter.value.as_str());
            return None;
        }
        let draft = match &mut self.draft {
            Some(draft) if draft.name == parameter.name && draft.base == parameter.value => draft,
            other => other.insert(Draft {
                name: parameter.name.clone(),
                base: parameter.value.clone(),
                text: parameter.value.clone(),
            }),
        };
        let id: Id = ui.make_persistent_id(("arc-option-bar", parameter.name.as_str()));
        // Enter is read before the field draws: the field gives up focus on Enter while drawing.
        let enter = ui.memory(|m| m.has_focus(id))
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let _field = ui
            .add(
                TextEdit::singleline(&mut draft.text)
                    .id(id)
                    .desired_width(120.0),
            )
            .labelled_by(label.id);
        enter.then(|| draft.text.trim().to_owned())
    }
}
