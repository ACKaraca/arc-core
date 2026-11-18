//! Localized strings (CORE-UI-024).
//!
//! Every visible string of this crate is a Fluent message identifier resolved through
//! [`Localize`]. The application's localizer implements the trait; this crate never parses
//! catalogs. [`Fallback`] puts an English source behind the active one, and [`resolve`] shows
//! the identifier itself when no source defines a message, so a gap is visible instead of blank.

use arc_base::error::Value;

/// A source of localized messages.
pub trait Localize {
    /// The message `id` formatted with `args`, or `None` when this source does not define it.
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String>;
}

/// The active language with English behind it: a message missing from `active` is taken from
/// `english`.
#[derive(Clone, Copy)]
pub struct Fallback<'a> {
    /// The active language.
    pub active: &'a dyn Localize,
    /// The English source.
    pub english: &'a dyn Localize,
}

impl Localize for Fallback<'_> {
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        self.active
            .message(id, args)
            .or_else(|| self.english.message(id, args))
    }
}

/// The text of message `id`, or the identifier itself when no source defines it.
#[must_use]
pub fn resolve(localize: &dyn Localize, id: &str, args: &[(&str, Value)]) -> String {
    localize.message(id, args).unwrap_or_else(|| id.to_owned())
}

/// A message this crate shows, identified for the catalog check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    /// The Fluent message identifier in `locales/en-US/arc-ui.ftl`.
    pub message_id: &'static str,
}

impl Message {
    /// The message's text in `localize` without arguments.
    #[must_use]
    pub fn text(self, localize: &dyn Localize) -> String {
        resolve(localize, self.message_id, &[])
    }
}

/// The messages this crate's widgets show.
pub mod messages {
    use super::Message;

    /// The accessible name of the command palette's search field.
    pub const PALETTE_SEARCH: Message = Message {
        message_id: "ui-palette-search",
    };
    /// Shown when the palette has no match.
    pub const PALETTE_NO_RESULTS: Message = Message {
        message_id: "ui-palette-no-results",
    };
    /// The button that closes a notification.
    pub const NOTIFICATION_DISMISS: Message = Message {
        message_id: "ui-notification-dismiss",
    };
    /// The name of the information severity.
    pub const SEVERITY_INFO: Message = Message {
        message_id: "ui-severity-info",
    };
    /// The name of the success severity.
    pub const SEVERITY_SUCCESS: Message = Message {
        message_id: "ui-severity-success",
    };
    /// The name of the warning severity.
    pub const SEVERITY_WARNING: Message = Message {
        message_id: "ui-severity-warning",
    };
    /// The name of the error severity.
    pub const SEVERITY_ERROR: Message = Message {
        message_id: "ui-severity-error",
    };
    /// The button that cancels a long operation.
    pub const PROGRESS_CANCEL: Message = Message {
        message_id: "ui-progress-cancel",
    };
    /// The accessible name of a dock separator.
    pub const DOCK_SEPARATOR: Message = Message {
        message_id: "ui-dock-separator",
    };
    /// The button that closes a dock panel.
    pub const DOCK_CLOSE: Message = Message {
        message_id: "ui-dock-close",
    };
    /// The command line's label while no command is active.
    pub const COMMAND_LINE: Message = Message {
        message_id: "ui-command-line",
    };
    /// Shown for a field whose selected objects differ.
    pub const PROPERTY_MIXED: Message = Message {
        message_id: "ui-property-mixed",
    };
    /// The marker of an overridden field.
    pub const PROPERTY_OVERRIDDEN: Message = Message {
        message_id: "ui-property-overridden",
    };
    /// The accessible name of a numeric field's scrub handle.
    pub const QUANTITY_SCRUB: Message = Message {
        message_id: "ui-quantity-scrub",
    };
    /// The label of the tree's filter field.
    pub const TREE_FILTER: Message = Message {
        message_id: "ui-tree-filter",
    };
    /// The button that expands a tree node.
    pub const TREE_EXPAND: Message = Message {
        message_id: "ui-tree-expand",
    };
    /// The button that collapses a tree node.
    pub const TREE_COLLAPSE: Message = Message {
        message_id: "ui-tree-collapse",
    };
    /// The visibility toggle of a tree node.
    pub const TREE_VISIBLE: Message = Message {
        message_id: "ui-tree-visible",
    };
    /// The lock toggle of a tree node.
    pub const TREE_LOCKED: Message = Message {
        message_id: "ui-tree-locked",
    };
    /// The label of the diagnostics filter field.
    pub const DIAGNOSTICS_FILTER: Message = Message {
        message_id: "ui-diagnostics-filter",
    };
    /// Shown when no diagnostic passes the filter.
    pub const DIAGNOSTICS_NONE: Message = Message {
        message_id: "ui-diagnostics-none",
    };
    /// The accessible name of a viewport, with the view's `name`.
    pub const VIEWPORT: Message = Message {
        message_id: "ui-viewport",
    };
    /// The selection count a viewport describes, with `count`.
    pub const VIEWPORT_SELECTION: Message = Message {
        message_id: "ui-viewport-selection",
    };
    /// A viewport's description while a command prompts, with `selection` and `prompt`.
    pub const VIEWPORT_STATUS: Message = Message {
        message_id: "ui-viewport-status",
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::{Fallback, Localize, messages, resolve};
    use arc_base::error::Value;

    /// A catalog of fixed texts; `{ $name }` is replaced by the argument's text.
    #[derive(Default)]
    pub struct Map(pub BTreeMap<String, String>);

    impl Map {
        pub fn of(entries: &[(&str, &str)]) -> Self {
            Self(
                entries
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            )
        }
    }

    impl Localize for Map {
        fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
            let mut text = self.0.get(id)?.clone();
            for (name, value) in args {
                let shown = match value {
                    Value::Text(t) => t.clone(),
                    Value::Int(i) => i.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                };
                text = text.replace(&format!("{{ ${name} }}"), &shown);
            }
            Some(text)
        }
    }

    #[test]
    fn missing_translation_falls_back_to_english_then_to_the_identifier() {
        let tr = Map::of(&[("ui-palette-search", "Komut ara")]);
        let en = Map::of(&[
            ("ui-palette-search", "Search commands"),
            ("ui-palette-no-results", "No matching command"),
        ]);
        let chain = Fallback {
            active: &tr,
            english: &en,
        };
        assert_eq!(messages::PALETTE_SEARCH.text(&chain), "Komut ara");
        assert_eq!(
            messages::PALETTE_NO_RESULTS.text(&chain),
            "No matching command"
        );
        assert_eq!(resolve(&chain, "ui-not-defined", &[]), "ui-not-defined");
    }

    #[test]
    fn every_message_is_in_the_english_catalog() {
        let catalog = include_str!("../../../locales/en-US/arc-ui.ftl");
        for m in [
            messages::PALETTE_SEARCH,
            messages::PALETTE_NO_RESULTS,
            messages::NOTIFICATION_DISMISS,
            messages::SEVERITY_INFO,
            messages::SEVERITY_SUCCESS,
            messages::SEVERITY_WARNING,
            messages::SEVERITY_ERROR,
            messages::PROGRESS_CANCEL,
            messages::DOCK_SEPARATOR,
            messages::DOCK_CLOSE,
            messages::COMMAND_LINE,
            messages::PROPERTY_MIXED,
            messages::PROPERTY_OVERRIDDEN,
            messages::QUANTITY_SCRUB,
            messages::TREE_FILTER,
            messages::TREE_EXPAND,
            messages::TREE_COLLAPSE,
            messages::TREE_VISIBLE,
            messages::TREE_LOCKED,
            messages::DIAGNOSTICS_FILTER,
            messages::DIAGNOSTICS_NONE,
            messages::VIEWPORT,
            messages::VIEWPORT_SELECTION,
            messages::VIEWPORT_STATUS,
        ] {
            assert!(
                catalog.contains(&format!("\n{} = ", m.message_id)),
                "{}",
                m.message_id
            );
        }
    }
}
