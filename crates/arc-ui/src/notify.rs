//! Notifications (CORE-UI-018) and progress with cancel (CORE-UI-019).
//!
//! [`Notifications`] keeps non-modal messages with a severity and optional actions in a bounded
//! history; an action is a command [`Intent`]. [`Progress`] decides from the caller's clock
//! whether an operation shows progress (after [`PROGRESS_DELAY_MILLIS`]) and a cancel control
//! (after [`CANCEL_DELAY_MILLIS`]).

use std::collections::VecDeque;

use arc_base::error::Value;
use egui::{Button, ProgressBar, Ui};
use serde::{Deserialize, Serialize};

use crate::command::Intent;
use crate::text::{Localize, Message, messages, resolve};
use crate::theme::Colors;

pub use arc_command::PROGRESS_DELAY_MILLIS;

/// Operations running at least this long show a cancel control.
pub const CANCEL_DELAY_MILLIS: u64 = 2000;
/// Notifications kept in the history; the oldest is dropped beyond it.
pub const MAX_HISTORY: usize = 200;
/// Actions kept per notification.
pub const MAX_ACTIONS: usize = 4;

/// How serious a notification is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Information.
    Info,
    /// Success.
    Success,
    /// Warning.
    Warning,
    /// Error.
    Error,
}

impl Severity {
    /// The severity's name, shown before the message so color is not the only signal.
    #[must_use]
    pub fn name(self) -> Message {
        match self {
            Self::Info => messages::SEVERITY_INFO,
            Self::Success => messages::SEVERITY_SUCCESS,
            Self::Warning => messages::SEVERITY_WARNING,
            Self::Error => messages::SEVERITY_ERROR,
        }
    }

    /// The color token of the severity.
    #[must_use]
    pub fn color(self, colors: &Colors) -> egui::Color32 {
        match self {
            Self::Info => colors.info,
            Self::Success => colors.success,
            Self::Warning => colors.warning,
            Self::Error => colors.error,
        }
    }
}

/// A button on a notification that requests a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// The Fluent message identifier of the button label.
    pub label: String,
    /// The command it requests.
    pub intent: Intent,
}

/// A notification's identifier, unique within one [`Notifications`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NotificationId(pub u64);

/// A non-modal message.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// The identifier.
    pub id: NotificationId,
    /// The severity.
    pub severity: Severity,
    /// The Fluent message identifier.
    pub message: String,
    /// The message arguments.
    pub args: Vec<(&'static str, Value)>,
    /// Up to [`MAX_ACTIONS`] actions.
    pub actions: Vec<Action>,
    /// When it was posted, on the caller's clock.
    pub posted_millis: u64,
    /// False once dismissed; it stays in the history.
    pub active: bool,
}

/// The notification history.
#[derive(Debug, Clone, Default)]
pub struct Notifications {
    history: VecDeque<Notification>,
    next: u64,
}

impl Notifications {
    /// Posts a notification and returns its identifier. Actions beyond [`MAX_ACTIONS`] are
    /// dropped; beyond [`MAX_HISTORY`] the oldest notification is dropped.
    pub fn post(
        &mut self,
        severity: Severity,
        message: impl Into<String>,
        args: Vec<(&'static str, Value)>,
        mut actions: Vec<Action>,
        now_millis: u64,
    ) -> NotificationId {
        actions.truncate(MAX_ACTIONS);
        let id = NotificationId(self.next);
        self.next = self.next.wrapping_add(1);
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Notification {
            id,
            severity,
            message: message.into(),
            args,
            actions,
            posted_millis: now_millis,
            active: true,
        });
        id
    }

    /// Dismisses `id`; returns false when it is not in the history.
    pub fn dismiss(&mut self, id: NotificationId) -> bool {
        self.history
            .iter_mut()
            .find(|n| n.id == id)
            .map(|n| n.active = false)
            .is_some()
    }

    /// The notifications not dismissed, oldest first.
    pub fn active(&self) -> impl Iterator<Item = &Notification> + '_ {
        self.history.iter().filter(|n| n.active)
    }

    /// Every kept notification, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &Notification> + '_ {
        self.history.iter()
    }

    /// Draws the active notifications with their severity name, text, actions and a dismiss
    /// button; returns the intent of a clicked action. Clicking an action also dismisses.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        colors: &Colors,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let mut intent = None;
        let mut dismissed = Vec::new();
        for n in self.active() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(n.severity.color(colors), n.severity.name().text(localize));
                    ui.label(resolve(localize, &n.message, &n.args));
                    for action in &n.actions {
                        if ui.button(resolve(localize, &action.label, &[])).clicked() {
                            intent = Some(action.intent.clone());
                            dismissed.push(n.id);
                        }
                    }
                    if ui
                        .add(Button::new(messages::NOTIFICATION_DISMISS.text(localize)))
                        .clicked()
                    {
                        dismissed.push(n.id);
                    }
                });
            });
        }
        for id in dismissed {
            self.dismiss(id);
        }
        intent
    }
}

/// What a running operation shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressView {
    /// Show the progress indicator.
    pub show_progress: bool,
    /// Show the cancel control.
    pub show_cancel: bool,
    /// The completed fraction in `[0, 1]`, or `None` when unknown.
    pub fraction: Option<f32>,
}

/// A running operation, timed on the caller's clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    started_millis: u64,
    fraction: Option<f32>,
    cancellable: bool,
}

impl Progress {
    /// An operation started at `now_millis`; `cancellable` when it observes a cancel token.
    #[must_use]
    pub fn start(now_millis: u64, cancellable: bool) -> Self {
        Self {
            started_millis: now_millis,
            fraction: None,
            cancellable,
        }
    }

    /// Records the completed fraction; values outside `[0, 1]` are clamped and a non-finite
    /// value makes the progress indeterminate.
    pub fn set_fraction(&mut self, fraction: f64) {
        #[expect(clippy::cast_possible_truncation, reason = "clamped to [0, 1]")]
        let clamped = fraction.clamp(0.0, 1.0) as f32;
        self.fraction = fraction.is_finite().then_some(clamped);
    }

    /// What to show at `now_millis`. A clock that runs backwards counts as no time passed.
    #[must_use]
    pub fn view(&self, now_millis: u64) -> ProgressView {
        let elapsed = now_millis.saturating_sub(self.started_millis);
        ProgressView {
            show_progress: elapsed >= PROGRESS_DELAY_MILLIS,
            show_cancel: self.cancellable && elapsed >= CANCEL_DELAY_MILLIS,
            fraction: self.fraction,
        }
    }

    /// Draws the progress row at `now_millis` when it is due; returns true when cancel was
    /// clicked.
    pub fn show(&self, ui: &mut Ui, now_millis: u64, localize: &dyn Localize) -> bool {
        let view = self.view(now_millis);
        if !view.show_progress {
            return false;
        }
        ui.horizontal(|ui| {
            let bar = match view.fraction {
                Some(f) => ProgressBar::new(f).show_percentage(),
                None => ProgressBar::new(0.0).animate(ui.style().animation_time > 0.0),
            };
            ui.add(bar);
            view.show_cancel
                && ui
                    .button(messages::PROGRESS_CANCEL.text(localize))
                    .clicked()
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(command: &str) -> Action {
        Action {
            label: "demo-open".into(),
            intent: Intent::new(command),
        }
    }

    #[test]
    fn history_is_bounded_and_dismiss_keeps_history() {
        let mut n = Notifications::default();
        let first = n.post(
            Severity::Info,
            "demo-a",
            vec![],
            vec![action("a.b.c"); 9],
            0,
        );
        assert_eq!(n.history().next().unwrap().actions.len(), MAX_ACTIONS);
        assert!(n.dismiss(first));
        assert_eq!(n.active().count(), 0);
        assert_eq!(n.history().count(), 1);
        for i in 0..MAX_HISTORY + 5 {
            n.post(Severity::Warning, "demo-b", vec![], vec![], i as u64);
        }
        assert_eq!(n.history().count(), MAX_HISTORY);
        assert!(!n.dismiss(first));
        assert!(n.history().all(|x| x.id != first));
    }

    #[test]
    fn progress_appears_after_300_ms_and_cancel_after_2_s() {
        let mut p = Progress::start(1000, true);
        assert!(!p.view(1000 + 299).show_progress);
        assert!(p.view(1000 + 300).show_progress);
        assert!(!p.view(1000 + 1999).show_cancel);
        assert!(p.view(1000 + 2000).show_cancel);
        assert!(
            !p.view(0).show_progress,
            "a clock running backwards shows nothing"
        );
        assert!(!Progress::start(0, false).view(10_000).show_cancel);
        p.set_fraction(1.5);
        assert_eq!(p.view(0).fraction, Some(1.0));
        p.set_fraction(f64::NAN);
        assert_eq!(p.view(0).fraction, None);
        p.set_fraction(-1e300);
        assert_eq!(p.view(0).fraction, Some(0.0));
    }

    #[test]
    fn severity_has_a_name_and_a_checked_color_token() {
        for id in crate::theme::ThemeId::ALL {
            let colors = id.theme().colors;
            let pairs = colors.pairs();
            for s in [
                Severity::Info,
                Severity::Success,
                Severity::Warning,
                Severity::Error,
            ] {
                assert!(s.name().message_id.starts_with("ui-severity-"));
                assert!(
                    pairs
                        .iter()
                        .any(|p| p.fg == s.color(&colors) && p.bg == colors.surface_raised)
                );
            }
        }
    }
}
