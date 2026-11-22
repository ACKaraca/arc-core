//! What a product's controller adds over the viewport: keys and typed text offered while the
//! viewport is the target, the context menu, the banner of a temporary display state, and the
//! display overrides that state asks for. The viewport panel and the frame composer call into
//! this module; nothing here knows a product.

use std::collections::HashSet;

use arc_base::ObjectId;
use arc_gpu::display::{DisplayOverrides, Isolation, Others, OverrideStyle};
use arc_ui::egui::{self, Ui};
use arc_ui::{Colors, LineAction, Localize, resolve};

use super::Action;
use super::interaction::{ContextEntry, Interaction, InteractionContext, KeyInput, KeyReply};

/// The most objects of one hide or isolation list the shell hands to the renderer; a longer
/// list is cut, which only affects the display.
const MAX_LISTED: usize = 50_000;

/// The temporary display state of one tab: what the controller hides and isolates.
#[derive(Debug, Default)]
pub(super) struct DisplayState {
    hidden: HashSet<ObjectId>,
    isolated: Option<HashSet<ObjectId>>,
    /// Whether the renderer has been told the current state.
    applied: bool,
}

impl DisplayState {
    /// Takes the controller's lists; true when they differ from the ones held.
    pub(super) fn update(&mut self, hidden: &[ObjectId], isolated: Option<&[ObjectId]>) -> bool {
        let hidden: HashSet<ObjectId> = hidden.iter().take(MAX_LISTED).copied().collect();
        let isolated: Option<HashSet<ObjectId>> =
            isolated.map(|l| l.iter().take(MAX_LISTED).copied().collect());
        if hidden == self.hidden && isolated == self.isolated {
            return false;
        }
        self.hidden = hidden;
        self.isolated = isolated;
        self.applied = false;
        true
    }

    /// Whether an object is shown, so it can be picked.
    pub(super) fn visible(&self, id: &ObjectId) -> bool {
        !self.hidden.contains(id) && self.isolated.as_ref().is_none_or(|set| set.contains(id))
    }

    /// Whether anything is hidden or isolated.
    pub(super) fn active(&self) -> bool {
        !self.hidden.is_empty() || self.isolated.is_some()
    }

    /// The overrides to send when the renderer has not been told the state yet.
    pub(super) fn pending(&mut self) -> Option<Option<DisplayOverrides>> {
        if self.applied {
            return None;
        }
        self.applied = true;
        Some(self.active().then(|| {
            DisplayOverrides {
                objects: self
                    .hidden
                    .iter()
                    .map(|id| (*id, OverrideStyle::hidden()))
                    .collect(),
                isolation: self.isolated.as_ref().map(|set| Isolation {
                    objects: set.iter().copied().collect(),
                    others: Others::Hide,
                }),
                ..DisplayOverrides::default()
            }
        }))
    }
}

/// Offers the keys and typed text of `events` to `controller`: Tab presses, key presses with the
/// command key that no binding took (the platform's copy, cut and paste events count as those),
/// and typed text. A consumed event leaves `events`; replaced text takes its place.
pub(super) fn offer_keys(
    controller: &mut dyn Interaction,
    cx: &InteractionContext<'_>,
    modifiers: egui::Modifiers,
    events: &mut Vec<egui::Event>,
    tab_only: bool,
) {
    let mut kept = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        let input = match &event {
            _ if tab_only
                && !matches!(
                    &event,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        ..
                    }
                ) =>
            {
                None
            }
            egui::Event::Text(text)
                if !text.is_empty()
                    && !text.chars().any(char::is_control)
                    && !modifiers.command =>
            {
                Some(KeyInput::text(text.clone()))
            }
            egui::Event::Key {
                key: egui::Key::Tab,
                pressed: true,
                modifiers: m,
                ..
            } if !(m.ctrl || m.alt || m.command) => {
                let mut input = KeyInput::key(egui::Key::Tab);
                input.modifiers = *m;
                Some(input)
            }
            // The platform reports the copy, cut and paste shortcuts as events of their own.
            egui::Event::Copy => Some(command_key(egui::Key::C)),
            egui::Event::Cut => Some(command_key(egui::Key::X)),
            egui::Event::Paste(_) => Some(command_key(egui::Key::V)),
            // A shortcut with the command key that no key binding took (copy, paste, ...).
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers: m,
                ..
            } if m.command && !m.alt && *key != egui::Key::Tab => {
                let mut input = KeyInput::key(*key);
                input.modifiers = *m;
                Some(input)
            }
            _ => None,
        };
        match input.map_or(KeyReply::Pass, |i| controller.key(cx, &i)) {
            KeyReply::Consumed => {}
            KeyReply::Text(text) => kept.push(egui::Event::Text(text)),
            KeyReply::Pass => kept.push(event),
        }
    }
    *events = kept;
}

/// The key `key` with the command key held.
fn command_key(key: egui::Key) -> KeyInput {
    let mut input = KeyInput::key(key);
    input.modifiers = egui::Modifiers::COMMAND;
    input
}

/// Paints the banner of a temporary display state: a frame around the viewport and a label at
/// its top left corner.
pub(super) fn paint_banner(ui: &Ui, rect: egui::Rect, text: &str, colors: &Colors) {
    let painter = ui.painter_at(rect);
    let color = colors.warning;
    painter.rect_stroke(
        rect.shrink(1.5),
        0.0,
        egui::Stroke::new(3.0, color),
        egui::StrokeKind::Inside,
    );
    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, colors.on_accent);
    let label = egui::Rect::from_min_size(
        rect.min + egui::vec2(3.0, 3.0),
        galley.size() + egui::vec2(14.0, 6.0),
    );
    painter.rect_filled(label, 2.0, color);
    painter.galley(label.min + egui::vec2(7.0, 3.0), galley, colors.on_accent);
}

/// Shows the context menu of `response` when it is open, with the entries `entries` returns now
/// (asked again each frame while the menu is open, so they follow the selection). Choosing an
/// entry pushes its action.
pub(super) fn context_menu(
    response: &egui::Response,
    localize: &dyn Localize,
    entries: &dyn Fn() -> Vec<ContextEntry>,
    actions: &mut Vec<Action>,
) {
    response.context_menu(|ui| {
        let list = entries();
        if list.is_empty() {
            ui.close();
            return;
        }
        for entry in list {
            if entry.separator {
                ui.separator();
                continue;
            }
            let label = resolve(localize, &entry.label, &[]);
            if ui
                .add_enabled(entry.enabled, egui::Button::new(label))
                .clicked()
            {
                let action: LineAction = entry.action;
                actions.push(Action::Line(action));
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::IdGenerator;

    fn ids(n: usize) -> Vec<ObjectId> {
        let mut generator = IdGenerator::seeded(5);
        (0..n).map(|_| generator.generate::<ObjectId>()).collect()
    }

    #[test]
    fn hidden_and_isolated_out_objects_are_not_shown_and_a_change_is_reported_once() {
        let list = ids(3);
        let mut state = DisplayState::default();
        assert!(!state.active() && list.iter().all(|id| state.visible(id)));
        assert!(!state.update(&[], None));
        assert!(state.update(&list[..1], None));
        assert!(!state.visible(&list[0]) && state.visible(&list[1]));
        assert!(state.pending().is_some_and(|o| o.is_some()));
        assert!(state.pending().is_none(), "the renderer is told once");
        assert!(!state.update(&list[..1], None));
        // An isolation shows only the listed objects.
        assert!(state.update(&[], Some(&list[1..2])));
        assert!(state.visible(&list[1]) && !state.visible(&list[0]) && !state.visible(&list[2]));
        // Reset: the renderer gets `None`.
        assert!(state.update(&[], None));
        assert_eq!(state.pending(), Some(None));
    }
}
