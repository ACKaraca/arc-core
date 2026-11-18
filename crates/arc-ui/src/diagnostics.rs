//! The diagnostics panel (CORE-UI-020).
//!
//! The panel lists a [`DiagnosticsSnapshot`] of a document's diagnostics list. [`filter`] keeps
//! the diagnostics of the checked severities whose localized message or code contains the
//! filter text (ignoring case). Activating a row with objects returns the navigation [`Intent`]
//! with the parameter `objects`.

use arc_doc::diagnostic::Diagnostic;
use arc_doc::diagnostics::DiagnosticsSnapshot;
use arc_doc::schema::Severity;
use egui::{Button, ScrollArea, Ui};

use crate::command::{Intent, object_list};
use crate::quantity::error_text;
use crate::text::{Localize, fold_case, messages};

/// Filter characters compared.
pub const MAX_FILTER_CHARS: usize = 256;

/// Which diagnostics the panel lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsFilter {
    /// List errors.
    pub errors: bool,
    /// List warnings.
    pub warnings: bool,
    /// The text the message or code contains.
    pub text: String,
}

impl Default for DiagnosticsFilter {
    fn default() -> Self {
        Self {
            errors: true,
            warnings: true,
            text: String::new(),
        }
    }
}

/// The indices of the diagnostics `filter` keeps, in list order.
#[must_use]
pub fn filter(
    diagnostics: &[Diagnostic],
    filter: &DiagnosticsFilter,
    localize: &dyn Localize,
) -> Vec<usize> {
    filter_in("", diagnostics, filter, localize)
}

/// [`filter`] with case folded by the rule of `language` (a BCP 47 tag).
#[must_use]
pub fn filter_in(
    language: &str,
    diagnostics: &[Diagnostic],
    filter: &DiagnosticsFilter,
    localize: &dyn Localize,
) -> Vec<usize> {
    let cut = |t: String| -> String { t.chars().take(MAX_FILTER_CHARS).collect() };
    let text = cut(fold_case(filter.text.trim(), language));
    // Error codes are ASCII identifiers: they match the plain lowercase query in every language.
    let code_text = cut(filter.text.trim().to_lowercase());
    diagnostics
        .iter()
        .enumerate()
        .filter(|(_, d)| match d.severity {
            Severity::Error => filter.errors,
            Severity::Warning => filter.warnings,
        })
        .filter(|(_, d)| {
            text.is_empty()
                || d.code().contains(&code_text)
                || fold_case(&error_text(&d.error, localize), language).contains(&text)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The navigation intent for `diagnostic`; `None` when it involves no object.
#[must_use]
pub fn navigate(diagnostic: &Diagnostic, command: &str) -> Option<Intent> {
    (!diagnostic.objects.is_empty())
        .then(|| Intent::new(command).with("objects", object_list(&diagnostic.objects)))
}

/// The diagnostics panel's state.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsPanel {
    /// The filter.
    pub filter: DiagnosticsFilter,
    /// The active language tag, which selects the case-folding rule; empty for none.
    pub language: String,
}

impl DiagnosticsPanel {
    /// Draws the severity check boxes, the filter field and the kept diagnostics, each with its
    /// severity name, and returns the navigation intent of an activated row.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snapshot: &DiagnosticsSnapshot,
        localize: &dyn Localize,
        navigate_command: &str,
    ) -> Option<Intent> {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.filter.errors,
                messages::SEVERITY_ERROR.text(localize),
            );
            ui.checkbox(
                &mut self.filter.warnings,
                messages::SEVERITY_WARNING.text(localize),
            );
            let label = ui.label(messages::DIAGNOSTICS_FILTER.text(localize));
            ui.text_edit_singleline(&mut self.filter.text)
                .labelled_by(label.id);
        });
        let kept = filter_in(
            &self.language,
            &snapshot.diagnostics,
            &self.filter,
            localize,
        );
        if kept.is_empty() {
            ui.label(messages::DIAGNOSTICS_NONE.text(localize));
            return None;
        }
        let mut intent = None;
        let height = ui.spacing().interact_size.y;
        ScrollArea::vertical().auto_shrink([false, true]).show_rows(
            ui,
            height,
            kept.len(),
            |ui, range| {
                for index in kept.get(range).unwrap_or_default() {
                    let Some(d) = snapshot.diagnostics.get(*index) else {
                        continue;
                    };
                    let (severity, color) = match d.severity {
                        Severity::Error => (messages::SEVERITY_ERROR, ui.visuals().error_fg_color),
                        Severity::Warning => {
                            (messages::SEVERITY_WARNING, ui.visuals().warn_fg_color)
                        }
                    };
                    let text = format!(
                        "{}: {}",
                        severity.text(localize),
                        error_text(&d.error, localize)
                    );
                    let row =
                        ui.add(Button::new(egui::RichText::new(text).color(color)).frame(false));
                    if row.clicked() {
                        intent = navigate(d, navigate_command);
                    }
                }
            },
        );
        intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tests::Map;
    use arc_base::id::Identifier;
    use arc_base::{Error, ObjectId};
    use arc_doc::diagnostic::Origin;

    fn diagnostics() -> Vec<Diagnostic> {
        let code = |c| arc_base::ErrorCode {
            code: c,
            category: arc_base::Category::InvalidInput,
            message_id: "demo-wall",
        };
        vec![
            Diagnostic::new(
                Severity::Error,
                Error::new(code("demo.wall.open")),
                vec![ObjectId::from_u128(7)],
                Origin::Validator,
            ),
            Diagnostic::new(
                Severity::Warning,
                Error::new(code("demo.wall.short")).with_arg("n", 3),
                vec![],
                Origin::Validator,
            ),
            Diagnostic::new(
                Severity::Warning,
                Error::new(code("demo.roof.flat")),
                vec![ObjectId::from_u128(8), ObjectId::from_u128(9)],
                Origin::Validator,
            ),
        ]
    }

    #[test]
    fn severity_and_text_filters_and_navigation() {
        let list = diagnostics();
        let catalog = Map::of(&[("demo-wall", "Wall { $n } has a gap")]);
        let mut f = DiagnosticsFilter::default();
        assert_eq!(filter(&list, &f, &catalog), [0, 1, 2]);
        f.errors = false;
        assert_eq!(filter(&list, &f, &catalog), [1, 2]);
        f.text = " ROOF ".into();
        assert_eq!(filter(&list, &f, &catalog), [2]);
        f.text = "wall 3".into();
        assert_eq!(filter(&list, &f, &catalog), [1]);
        f.warnings = false;
        assert!(filter(&list, &f, &catalog).is_empty());
        let intent = navigate(&list[2], "core.view.show_objects").unwrap();
        assert_eq!(intent.command, "core.view.show_objects");
        assert_eq!(
            intent.params["objects"],
            object_list(&[ObjectId::from_u128(8), ObjectId::from_u128(9)])
        );
        assert_eq!(navigate(&list[1], "core.view.show_objects"), None);
    }
}
