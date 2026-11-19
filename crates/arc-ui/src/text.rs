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

/// Whether `language` (a BCP 47 tag such as `tr-TR`) is Turkish or Azerbaijani, the languages
/// with a dotted and a dotless `i`.
fn is_dotted_i_language(language: &str) -> bool {
    let primary = language.split(['-', '_']).next().unwrap_or_default();
    primary.eq_ignore_ascii_case("tr") || primary.eq_ignore_ascii_case("az")
}

/// `text` folded for case-insensitive comparison in `language`. In Turkish and Azerbaijani `I`
/// folds to `ı` and `İ` to `i`, so `ı`/`I` and `i`/`İ` match as users expect; in every other
/// language this is Unicode lowercasing. The result is itself unchanged by folding again, and
/// there is no global state: the language is an input.
#[must_use]
pub fn fold_case(text: &str, language: &str) -> String {
    if !is_dotted_i_language(language) {
        return text.to_lowercase();
    }
    text.chars()
        .flat_map(|c| match c {
            'I' => vec!['ı'],
            'İ' => vec!['i'],
            other => other.to_lowercase().collect(),
        })
        .collect()
}

/// Whether `name` contains `filter` when both are folded in `language`; an empty filter keeps
/// every name. Tree sources use it to apply the filter text of a [`crate::tree::TreeView`].
#[must_use]
pub fn matches_filter(name: &str, filter: &str, language: &str) -> bool {
    fold_case(name, language).contains(&fold_case(filter.trim(), language))
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
    /// The accessible name of the view cube.
    pub const VIEW_CUBE: Message = Message {
        message_id: "ui-view-cube",
    };
    /// The name of the top face of the view cube.
    pub const VIEW_TOP: Message = Message {
        message_id: "ui-view-cube-top",
    };
    /// The name of the bottom face of the view cube.
    pub const VIEW_BOTTOM: Message = Message {
        message_id: "ui-view-cube-bottom",
    };
    /// The name of the front face of the view cube.
    pub const VIEW_FRONT: Message = Message {
        message_id: "ui-view-cube-front",
    };
    /// The name of the back face of the view cube.
    pub const VIEW_BACK: Message = Message {
        message_id: "ui-view-cube-back",
    };
    /// The name of the left face of the view cube.
    pub const VIEW_LEFT: Message = Message {
        message_id: "ui-view-cube-left",
    };
    /// The name of the right face of the view cube.
    pub const VIEW_RIGHT: Message = Message {
        message_id: "ui-view-cube-right",
    };
    /// The name of a view cube edge, with the face names `first` and `second`.
    pub const VIEW_EDGE: Message = Message {
        message_id: "ui-view-cube-edge",
    };
    /// The name of a view cube corner, with the face names `first`, `second` and `third`.
    pub const VIEW_CORNER: Message = Message {
        message_id: "ui-view-cube-corner",
    };
    /// The accessible name of a table.
    pub const TABLE: Message = Message {
        message_id: "ui-table",
    };
    /// The accessible name of a sorted column header, with `name` and `direction`.
    pub const TABLE_SORTED: Message = Message {
        message_id: "ui-table-sorted",
    };
    /// The sort direction name for an ascending sort.
    pub const TABLE_ASCENDING: Message = Message {
        message_id: "ui-table-ascending",
    };
    /// The sort direction name for a descending sort.
    pub const TABLE_DESCENDING: Message = Message {
        message_id: "ui-table-descending",
    };
    /// Shown when a paste refused cells, with `count`.
    pub const TABLE_PASTE_REFUSED: Message = Message {
        message_id: "ui-table-paste-refused",
    };
    /// Shown when the selection is too large to copy.
    pub const TABLE_COPY_TOO_LARGE: Message = Message {
        message_id: "ui-table-copy-too-large",
    };
    /// The accessible name of the color picker.
    pub const STYLE_COLOR: Message = Message {
        message_id: "ui-style-color",
    };
    /// The accessible name of the layer picker.
    pub const STYLE_LAYER: Message = Message {
        message_id: "ui-style-layer",
    };
    /// The accessible name of the linetype picker.
    pub const STYLE_LINETYPE: Message = Message {
        message_id: "ui-style-linetype",
    };
    /// The accessible name of the lineweight picker.
    pub const STYLE_LINEWEIGHT: Message = Message {
        message_id: "ui-style-lineweight",
    };
    /// The accessible name of the material picker.
    pub const STYLE_MATERIAL: Message = Message {
        message_id: "ui-style-material",
    };
    /// The label of the style pickers' filter field.
    pub const STYLE_FILTER: Message = Message {
        message_id: "ui-style-filter",
    };
    /// The entry that offers a true color, with the typed `hex` color.
    pub const STYLE_TRUE_COLOR: Message = Message {
        message_id: "ui-style-true-color",
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
    fn turkish_folding_pairs_dotted_and_dotless_i() {
        use super::{fold_case, matches_filter};
        assert_eq!(fold_case("IĞDIR İZMİR", "tr-TR"), "ığdır izmir");
        assert_eq!(
            fold_case("IĞDIR İZMİR", "en-US"),
            "iğdir i\u{307}zmi\u{307}r"
        );
        assert!(matches_filter("İzmir", "izm", "tr"));
        assert!(matches_filter("Işık", "ışık", "tr_TR"));
        assert!(!matches_filter("Işık", "isik", "tr"));
        assert!(matches_filter("Image", "image", "en-US"));
    }

    proptest::proptest! {
        #[test]
        fn folding_is_idempotent_and_total(text in "\\PC{0,60}", lang in "(tr|az|en|de|tr-TR|[a-z]{2,3})") {
            let once = super::fold_case(&text, &lang);
            proptest::prop_assert_eq!(super::fold_case(&once, &lang), once);
        }

        #[test]
        fn ascii_is_plain_lowercase_outside_turkic(text in "[ -~]{0,60}", lang in "(en|de|fr|en-US|zz|)") {
            proptest::prop_assert_eq!(super::fold_case(&text, &lang), text.to_ascii_lowercase());
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
            messages::VIEW_CUBE,
            messages::VIEW_TOP,
            messages::VIEW_BOTTOM,
            messages::VIEW_FRONT,
            messages::VIEW_BACK,
            messages::VIEW_LEFT,
            messages::VIEW_RIGHT,
            messages::VIEW_EDGE,
            messages::VIEW_CORNER,
            messages::TABLE,
            messages::TABLE_SORTED,
            messages::TABLE_ASCENDING,
            messages::TABLE_DESCENDING,
            messages::TABLE_PASTE_REFUSED,
            messages::TABLE_COPY_TOO_LARGE,
            messages::STYLE_COLOR,
            messages::STYLE_LAYER,
            messages::STYLE_LINETYPE,
            messages::STYLE_LINEWEIGHT,
            messages::STYLE_MATERIAL,
            messages::STYLE_FILTER,
            messages::STYLE_TRUE_COLOR,
        ] {
            assert!(
                catalog.contains(&format!("\n{} = ", m.message_id)),
                "{}",
                m.message_id
            );
        }
    }
}
