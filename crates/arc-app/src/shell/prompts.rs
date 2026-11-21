//! What the shell draws above the dock: the start-up recovery prompt (CORE-APP-006) and the
//! active document's banners (CORE-APP-005). Both only collect the user's choices as
//! [`Choice`]s; the shell carries them out after the frame.

use arc_base::error::Value;
use arc_doc::package::recovery::Orphan;
use arc_ui::egui::{self, Ui};
use arc_ui::{Localize, Message, resolve};

use crate::application::{Banner, Comparison, Tab, TabId};

const TITLE: Message = Message {
    message_id: "app-recovery-title",
};
const ENTRY: Message = Message {
    message_id: "app-recovery-entry",
};
pub(super) const UNTITLED: Message = Message {
    message_id: "app-recovery-untitled",
};
const UNREADABLE: Message = Message {
    message_id: "app-recovery-unreadable",
};
const RECOVER: Message = Message {
    message_id: "app-recovery-recover",
};
const COMPARE: Message = Message {
    message_id: "app-recovery-compare",
};
const DISCARD: Message = Message {
    message_id: "app-recovery-discard",
};
const LATER: Message = Message {
    message_id: "app-recovery-later",
};
const BASE: Message = Message {
    message_id: "app-recovery-compare-base",
};
const NO_BASE: Message = Message {
    message_id: "app-recovery-compare-no-base",
};
const UNCHANGED: Message = Message {
    message_id: "app-recovery-compare-unchanged",
};
const CHANGED: Message = Message {
    message_id: "app-recovery-compare-changed",
};
const OBJECTS: Message = Message {
    message_id: "app-recovery-compare-objects",
};
const RECORDS: Message = Message {
    message_id: "app-recovery-compare-records",
};
const CLOSE_SAVE: Message = Message {
    message_id: "app-close-save",
};
const CLOSE_DISCARD: Message = Message {
    message_id: "app-close-discard",
};
const CLOSE_CANCEL: Message = Message {
    message_id: "app-close-cancel",
};
const UNSAVED: Message = Message {
    message_id: "app-dialog-unsaved",
};
const TAKE_OVER: Message = Message {
    message_id: "app-banner-take-over",
};

/// The banner whose cause the user can remove by taking over the edit lock.
pub const STALE_LOCK: &str = "app-banner-stale-lock";

/// A recoverable session in the prompt.
#[derive(Debug)]
pub struct RecoveryEntry {
    /// The session as the recovery scan found it.
    pub orphan: Orphan,
    /// What [`Application::compare`](crate::application::Application::compare) reported, once
    /// the user asked for it.
    pub comparison: Option<Comparison>,
}

/// A choice made in the prompt or on a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Recover the entry at this position.
    Recover(usize),
    /// Compare the entry at this position with its base.
    Compare(usize),
    /// Discard the entry at this position.
    Discard(usize),
    /// Close the prompt; the sessions stay for the next start.
    Later,
    /// Take over the stale edit lock of the tab.
    TakeOver(TabId),
    /// The answer to the close question of the window.
    Close(crate::application::CloseAnswer),
}

fn count(n: u64) -> Value {
    i64::try_from(n).unwrap_or(i64::MAX).into()
}

/// The name of an entry: its base file's name, or the untitled text.
fn name(orphan: &Orphan, localize: &dyn Localize) -> String {
    orphan
        .base
        .as_ref()
        .and_then(|b| {
            std::path::Path::new(&b.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| UNTITLED.text(localize))
}

fn comparison(ui: &mut Ui, c: &Comparison, localize: &dyn Localize) {
    match &c.base {
        Some(path) => {
            let path = path.display().to_string();
            ui.label(resolve(
                localize,
                BASE.message_id,
                &[("path", path.as_str().into())],
            ));
            let state = if c.base_unchanged { UNCHANGED } else { CHANGED };
            ui.label(state.text(localize));
        }
        None => {
            ui.label(NO_BASE.text(localize));
        }
    }
    if let Some(objects) = c.base_objects {
        ui.label(resolve(
            localize,
            OBJECTS.message_id,
            &[("objects", count(objects))],
        ));
    }
    ui.label(resolve(
        localize,
        RECORDS.message_id,
        &[("records", count(c.records))],
    ));
}

/// Draws the recovery prompt as a modal over the frame while `entries` is not empty.
pub fn recovery(
    ui: &Ui,
    entries: &[RecoveryEntry],
    localize: &dyn Localize,
    choices: &mut Vec<Choice>,
) {
    if entries.is_empty() {
        return;
    }
    egui::Modal::new(egui::Id::unique("arc-app-recovery")).show(ui.ctx(), |ui| {
        ui.heading(TITLE.text(localize));
        egui::ScrollArea::vertical()
            .max_height(400.0)
            .show(ui, |ui| {
                for (at, entry) in entries.iter().enumerate() {
                    ui.separator();
                    let o = &entry.orphan;
                    let text = resolve(
                        localize,
                        ENTRY.message_id,
                        &[
                            ("name", name(o, localize).as_str().into()),
                            ("records", count(o.records)),
                        ],
                    );
                    ui.label(text);
                    ui.horizontal(|ui| {
                        if o.problem.is_some() {
                            ui.label(UNREADABLE.text(localize));
                        } else {
                            if ui.button(RECOVER.text(localize)).clicked() {
                                choices.push(Choice::Recover(at));
                            }
                            if ui.button(COMPARE.text(localize)).clicked() {
                                choices.push(Choice::Compare(at));
                            }
                        }
                        if ui.button(DISCARD.text(localize)).clicked() {
                            choices.push(Choice::Discard(at));
                        }
                    });
                    if let Some(c) = &entry.comparison {
                        comparison(ui, c, localize);
                    }
                }
            });
        ui.separator();
        if ui.button(LATER.text(localize)).clicked() {
            choices.push(Choice::Later);
        }
    });
}

fn banner_text(banner: &Banner, localize: &dyn Localize) -> String {
    let args: Vec<(&str, Value)> = banner
        .arguments
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    resolve(localize, banner.message_id, &args)
}

/// Draws the banners of `tab` in a strip at the top of the frame; nothing without banners.
pub fn banners(ui: &mut Ui, tab: &Tab, localize: &dyn Localize, choices: &mut Vec<Choice>) {
    if tab.banners().is_empty() {
        return;
    }
    egui::Panel::top("arc-app-banners").show(ui, |ui| {
        for banner in tab.banners() {
            ui.horizontal_wrapped(|ui| {
                ui.label(banner_text(banner, localize));
                if banner.message_id == STALE_LOCK && ui.button(TAKE_OVER.text(localize)).clicked()
                {
                    choices.push(Choice::TakeOver(tab.id()));
                }
            });
        }
    });
}

/// Draws the question about closing the document `name` with unsaved changes as a modal: Save,
/// Discard or Cancel (Escape cancels). The shell asks it when the platform has no native dialog.
pub fn close_question(ui: &Ui, name: &str, localize: &dyn Localize, choices: &mut Vec<Choice>) {
    use crate::application::CloseAnswer;
    let modal = egui::Modal::new(egui::Id::unique("arc-app-close")).show(ui.ctx(), |ui| {
        ui.heading(name);
        ui.label(UNSAVED.text(localize));
        ui.separator();
        ui.horizontal(|ui| {
            for (message, answer) in [
                (CLOSE_SAVE, CloseAnswer::Save),
                (CLOSE_DISCARD, CloseAnswer::Discard),
                (CLOSE_CANCEL, CloseAnswer::Cancel),
            ] {
                if ui.button(message.text(localize)).clicked() {
                    choices.push(Choice::Close(answer));
                }
            }
        });
    });
    if modal.should_close() && choices.is_empty() {
        choices.push(Choice::Close(CloseAnswer::Cancel));
    }
}
