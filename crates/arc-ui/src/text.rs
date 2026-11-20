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

/// The direction of a layout: the order of panels, columns and controls, and the side text
/// starts from (CORE-UI-025).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Logical start on the left (the default).
    #[default]
    LeftToRight,
    /// Logical start on the right.
    RightToLeft,
}

/// Primary language subtags of scripts written right to left.
const RIGHT_TO_LEFT_LANGUAGES: [&str; 11] = [
    "ar", "ckb", "dv", "fa", "he", "iw", "ps", "sd", "ug", "ur", "yi",
];

/// Script subtags written right to left, for tags such as `ku-Arab`.
const RIGHT_TO_LEFT_SCRIPTS: [&str; 4] = ["Arab", "Hebr", "Thaa", "Syrc"];

impl Direction {
    /// The direction of a BCP 47 `language` tag: right to left for the Arabic, Hebrew, Persian,
    /// Urdu and similar languages, or for a right-to-left script subtag, unless the tag names the
    /// Latin script. An empty tag is left to right.
    #[must_use]
    pub fn of_language(language: &str) -> Self {
        let mut subtags = language.split(['-', '_']);
        let primary = subtags.next().unwrap_or_default();
        let rest: Vec<&str> = subtags.collect();
        if rest.iter().any(|s| s.eq_ignore_ascii_case("Latn")) {
            return Self::LeftToRight;
        }
        let by_language = RIGHT_TO_LEFT_LANGUAGES
            .iter()
            .any(|l| primary.eq_ignore_ascii_case(l));
        let by_script = rest.iter().any(|s| {
            RIGHT_TO_LEFT_SCRIPTS
                .iter()
                .any(|r| s.eq_ignore_ascii_case(r))
        });
        if by_language || by_script {
            Self::RightToLeft
        } else {
            Self::LeftToRight
        }
    }

    /// The direction a widget uses: the explicit `override_direction` when one is given,
    /// otherwise the direction of `language`.
    #[must_use]
    pub fn resolve(language: &str, override_direction: Option<Self>) -> Self {
        override_direction.unwrap_or_else(|| Self::of_language(language))
    }

    /// Whether the logical start is on the right.
    #[must_use]
    pub fn is_right_to_left(self) -> bool {
        self == Self::RightToLeft
    }

    /// The alignment of text that starts at the logical start: left in left-to-right layouts,
    /// right in right-to-left ones.
    #[must_use]
    pub fn start_align(self) -> egui::Align2 {
        match self {
            Self::LeftToRight => egui::Align2::LEFT_CENTER,
            Self::RightToLeft => egui::Align2::RIGHT_CENTER,
        }
    }

    /// The egui layout whose main direction runs from the logical start, with the given
    /// cross alignment.
    #[must_use]
    pub fn layout(self, cross_align: egui::Align) -> egui::Layout {
        match self {
            Self::LeftToRight => egui::Layout::left_to_right(cross_align),
            Self::RightToLeft => egui::Layout::right_to_left(cross_align),
        }
    }

    /// Lays out `add_contents` in one row that runs from the logical start and takes the width
    /// the parent has left, like `Ui::horizontal`; with `wrap` the row continues on the next
    /// line, like `Ui::horizontal_wrapped`.
    pub fn row<R>(
        self,
        ui: &mut egui::Ui,
        wrap: bool,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let size = egui::vec2(
            ui.available_size_before_wrap().x,
            ui.spacing().interact_size.y,
        );
        let rect = egui::Rect::from_min_size(ui.cursor().min, size);
        let layout = self.layout(egui::Align::Center).with_main_wrap(wrap);
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rect).layout(layout),
            add_contents,
        )
    }
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
    /// A numeric field's value is outside the range, naming the field.
    pub const QUANTITY_RANGE_FIELD: Message = Message {
        message_id: "ui-quantity-out-of-range-field",
    };
    /// A numeric field's text is no value; says what is accepted.
    pub const QUANTITY_EXPECTED: Message = Message {
        message_id: "ui-quantity-expected",
    };
    /// [`QUANTITY_EXPECTED`], naming the field.
    pub const QUANTITY_EXPECTED_FIELD: Message = Message {
        message_id: "ui-quantity-expected-field",
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
    /// The accessible name of a column header that is one of several sort keys, with `name`,
    /// `direction`, `key` (its 1-based position) and `keys` (the number of keys).
    pub const TABLE_SORTED_KEY: Message = Message {
        message_id: "ui-table-sorted-key",
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
    /// The accessible name of an in-place cell editor.
    pub const TABLE_EDIT: Message = Message {
        message_id: "ui-table-edit",
    };
    /// The accessible name of a table's horizontal scroll bar.
    pub const TABLE_SCROLL_HORIZONTAL: Message = Message {
        message_id: "ui-table-scroll-horizontal",
    };
    /// The accessible name of a table's vertical scroll bar.
    pub const TABLE_SCROLL_VERTICAL: Message = Message {
        message_id: "ui-table-scroll-vertical",
    };
    /// The accessible name of the tool option bar.
    pub const OPTION_BAR: Message = Message {
        message_id: "ui-option-bar",
    };
    /// The accessible name of the dynamic input panel.
    pub const DYNAMIC_INPUT: Message = Message {
        message_id: "ui-dynamic-input",
    };
    /// The label of the distance field of the dynamic input.
    pub const DYNAMIC_DISTANCE: Message = Message {
        message_id: "ui-dynamic-distance",
    };
    /// The label of the angle field of the dynamic input.
    pub const DYNAMIC_ANGLE: Message = Message {
        message_id: "ui-dynamic-angle",
    };
    /// The name of an endpoint snap.
    pub const SNAP_ENDPOINT: Message = Message {
        message_id: "ui-snap-endpoint",
    };
    /// The name of a midpoint snap.
    pub const SNAP_MIDPOINT: Message = Message {
        message_id: "ui-snap-midpoint",
    };
    /// The name of a center snap.
    pub const SNAP_CENTER: Message = Message {
        message_id: "ui-snap-center",
    };
    /// The name of an intersection snap.
    pub const SNAP_INTERSECTION: Message = Message {
        message_id: "ui-snap-intersection",
    };
    /// The name of a perpendicular snap.
    pub const SNAP_PERPENDICULAR: Message = Message {
        message_id: "ui-snap-perpendicular",
    };
    /// The name of a tangent snap.
    pub const SNAP_TANGENT: Message = Message {
        message_id: "ui-snap-tangent",
    };
    /// The name of a nearest snap.
    pub const SNAP_NEAREST: Message = Message {
        message_id: "ui-snap-nearest",
    };
    /// The name of a node snap.
    pub const SNAP_NODE: Message = Message {
        message_id: "ui-snap-node",
    };
    /// The name of a quadrant snap.
    pub const SNAP_QUADRANT: Message = Message {
        message_id: "ui-snap-quadrant",
    };
}

#[cfg(test)]
mod direction_tests {
    use super::Direction;
    use egui::Align2;

    #[test]
    fn the_language_gives_the_direction_of_its_script() {
        assert_eq!(Direction::of_language("ar"), Direction::RightToLeft);
        assert_eq!(Direction::of_language("he-IL"), Direction::RightToLeft);
        assert_eq!(Direction::of_language("fa_IR"), Direction::RightToLeft);
        assert_eq!(Direction::of_language("ku-Arab"), Direction::RightToLeft);
        assert_eq!(Direction::of_language("az-Arab"), Direction::RightToLeft);
        assert_eq!(Direction::of_language("ar-Latn"), Direction::LeftToRight);
        assert_eq!(Direction::of_language("tr-TR"), Direction::LeftToRight);
        assert_eq!(Direction::of_language("en-US"), Direction::LeftToRight);
        assert_eq!(Direction::of_language(""), Direction::LeftToRight);
    }

    #[test]
    fn an_explicit_direction_overrides_the_language() {
        assert_eq!(
            Direction::resolve("ar", Some(Direction::LeftToRight)),
            Direction::LeftToRight
        );
        assert_eq!(Direction::resolve("en", None), Direction::LeftToRight);
        assert_eq!(Direction::resolve("he", None), Direction::RightToLeft);
    }

    #[test]
    fn text_starts_at_the_logical_start() {
        assert_eq!(Direction::LeftToRight.start_align(), Align2::LEFT_CENTER);
        assert_eq!(Direction::RightToLeft.start_align(), Align2::RIGHT_CENTER);
    }
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
            messages::QUANTITY_RANGE_FIELD,
            messages::QUANTITY_EXPECTED,
            messages::QUANTITY_EXPECTED_FIELD,
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
            messages::TABLE_SORTED_KEY,
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
            messages::TABLE_EDIT,
            messages::TABLE_SCROLL_HORIZONTAL,
            messages::TABLE_SCROLL_VERTICAL,
            messages::OPTION_BAR,
            messages::DYNAMIC_INPUT,
            messages::DYNAMIC_DISTANCE,
            messages::DYNAMIC_ANGLE,
            messages::SNAP_ENDPOINT,
            messages::SNAP_MIDPOINT,
            messages::SNAP_CENTER,
            messages::SNAP_INTERSECTION,
            messages::SNAP_PERPENDICULAR,
            messages::SNAP_TANGENT,
            messages::SNAP_NEAREST,
            messages::SNAP_NODE,
            messages::SNAP_QUADRANT,
        ] {
            assert!(
                catalog.contains(&format!("\n{} = ", m.message_id)),
                "{}",
                m.message_id
            );
        }
    }
}
