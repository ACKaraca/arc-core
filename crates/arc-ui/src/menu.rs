//! Menus, toolbars and context menus from data (CORE-UI-003).
//!
//! A [`Menu`] or [`Toolbar`] is data that names commands by identifier; deserialization
//! validates it. [`Lookup::menu`] and [`Lookup::toolbar`] resolve it against the registry into
//! entries with localized labels, binding text and disabled reasons. The `show` functions draw
//! entries with egui and return the [`Intent`] of the clicked command; they run nothing.

use arc_base::{Error, Result};
use arc_command::spec::is_command_id;
use egui::{Button, Role, Ui, Vec2, WidgetInfo};
use serde::{Deserialize, Serialize};

use crate::command::{CommandView, Intent, Lookup};
use crate::error::codes;
use crate::icons::Icons;

/// The deepest nesting of submenus; the top level is depth 1.
pub const MAX_DEPTH: usize = 8;
/// The most items in one menu or toolbar definition, counted over every level.
pub const MAX_ITEMS: usize = 4096;
/// The longest label message identifier, in bytes.
pub const MAX_LABEL_BYTES: usize = 128;

/// An item of a menu definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MenuItem {
    /// A command.
    Command {
        /// The command identifier.
        command: String,
    },
    /// A submenu.
    Submenu {
        /// The Fluent message identifier of the label.
        label: String,
        /// The items.
        items: Vec<MenuItem>,
    },
    /// A separator line.
    Separator,
}

/// A validated menu definition: a menu bar (its top-level items are usually submenus) or a
/// context menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MenuRaw", into = "MenuRaw")]
pub struct Menu {
    items: Vec<MenuItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MenuRaw {
    items: Vec<MenuItem>,
}

/// An item of a toolbar definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolbarItem {
    /// A command button.
    Command {
        /// The command identifier.
        command: String,
        /// The icon name in the atlas; without one the button shows the label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    /// A separator.
    Separator,
}

/// A validated toolbar definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ToolbarRaw", into = "ToolbarRaw")]
pub struct Toolbar {
    items: Vec<ToolbarItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolbarRaw {
    items: Vec<ToolbarItem>,
}

fn malformed(reason: &'static str, value: &str) -> Error {
    Error::new(codes::MENU_INVALID)
        .with_arg("value", value)
        .with_detail("reason", reason)
}

fn is_message_id(id: &str) -> bool {
    id.len() <= MAX_LABEL_BYTES
        && id.starts_with(|c: char| c.is_ascii_alphabetic())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_command(command: &str) -> Result<()> {
    if is_command_id(command) {
        Ok(())
    } else {
        Err(malformed("command", command))
    }
}

fn check_items(items: &[MenuItem], depth: usize, count: &mut usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::new(codes::MENU_TOO_DEEP)
            .with_detail("depth", i64::try_from(depth).unwrap_or(i64::MAX)));
    }
    for item in items {
        *count += 1;
        if *count > MAX_ITEMS {
            return Err(Error::new(codes::MENU_TOO_LARGE));
        }
        match item {
            MenuItem::Command { command } => check_command(command)?,
            MenuItem::Submenu { label, items } => {
                if !is_message_id(label) {
                    return Err(malformed("label", label));
                }
                check_items(items, depth + 1, count)?;
            }
            MenuItem::Separator => {}
        }
    }
    Ok(())
}

impl Menu {
    /// A menu of `items`.
    ///
    /// # Errors
    /// `ui.menu.invalid` for a malformed command identifier or label message identifier,
    /// `ui.menu.too_deep` beyond [`MAX_DEPTH`] levels, `ui.menu.too_large` beyond
    /// [`MAX_ITEMS`] items.
    pub fn new(items: Vec<MenuItem>) -> Result<Self> {
        check_items(&items, 1, &mut 0)?;
        Ok(Self { items })
    }

    /// The items.
    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }
}

impl TryFrom<MenuRaw> for Menu {
    type Error = Error;
    fn try_from(raw: MenuRaw) -> Result<Self> {
        Self::new(raw.items)
    }
}

impl From<Menu> for MenuRaw {
    fn from(menu: Menu) -> Self {
        Self { items: menu.items }
    }
}

impl Toolbar {
    /// A toolbar of `items`.
    ///
    /// # Errors
    /// `ui.menu.invalid` for a malformed command identifier or icon name,
    /// `ui.menu.too_large` beyond [`MAX_ITEMS`] items.
    pub fn new(items: Vec<ToolbarItem>) -> Result<Self> {
        if items.len() > MAX_ITEMS {
            return Err(Error::new(codes::MENU_TOO_LARGE));
        }
        for item in &items {
            if let ToolbarItem::Command { command, icon } = item {
                check_command(command)?;
                if let Some(icon) = icon
                    && (icon.is_empty()
                        || icon.len() > crate::icons::MAX_NAME_BYTES
                        || !icon
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
                {
                    return Err(malformed("icon", icon));
                }
            }
        }
        Ok(Self { items })
    }

    /// The items.
    #[must_use]
    pub fn items(&self) -> &[ToolbarItem] {
        &self.items
    }
}

impl TryFrom<ToolbarRaw> for Toolbar {
    type Error = Error;
    fn try_from(raw: ToolbarRaw) -> Result<Self> {
        Self::new(raw.items)
    }
}

impl From<Toolbar> for ToolbarRaw {
    fn from(toolbar: Toolbar) -> Self {
        Self {
            items: toolbar.items,
        }
    }
}

/// A resolved menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A command.
    Command(CommandView),
    /// A submenu with its localized label.
    Submenu {
        /// The localized label.
        label: String,
        /// The entries.
        entries: Vec<Entry>,
    },
    /// A separator.
    Separator,
}

/// A resolved toolbar entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEntry {
    /// A command button.
    Command {
        /// The command.
        view: CommandView,
        /// The icon name.
        icon: Option<String>,
    },
    /// A separator.
    Separator,
}

impl Lookup<'_> {
    /// Resolves `menu`. Items naming a command that is not registered for the UI surface are
    /// left out and reported with `ui.command.unknown`; the rest of the menu still resolves.
    #[must_use]
    pub fn menu(&self, menu: &Menu) -> (Vec<Entry>, Vec<Error>) {
        let mut problems = Vec::new();
        let entries = self.entries(&menu.items, &mut problems);
        (entries, problems)
    }

    fn entries(&self, items: &[MenuItem], problems: &mut Vec<Error>) -> Vec<Entry> {
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Command { command } => match self.view(command) {
                    Ok(view) => Some(Entry::Command(view)),
                    Err(error) => {
                        problems.push(error);
                        None
                    }
                },
                MenuItem::Submenu { label, items } => Some(Entry::Submenu {
                    label: crate::text::resolve(self.localize, label, &[]),
                    entries: self.entries(items, problems),
                }),
                MenuItem::Separator => Some(Entry::Separator),
            })
            .collect()
    }

    /// Resolves `toolbar` like [`Lookup::menu`].
    #[must_use]
    pub fn toolbar(&self, toolbar: &Toolbar) -> (Vec<ToolEntry>, Vec<Error>) {
        let mut problems = Vec::new();
        let entries = toolbar
            .items
            .iter()
            .filter_map(|item| match item {
                ToolbarItem::Command { command, icon } => match self.view(command) {
                    Ok(view) => Some(ToolEntry::Command {
                        view,
                        icon: icon.clone(),
                    }),
                    Err(error) => {
                        problems.push(error);
                        None
                    }
                },
                ToolbarItem::Separator => Some(ToolEntry::Separator),
            })
            .collect();
        (entries, problems)
    }
}

fn command_button(ui: &mut Ui, view: &CommandView) -> Option<Intent> {
    let mut button = Button::new(view.label.as_str());
    if let Some(binding) = &view.binding {
        button = button.shortcut_text(binding.as_str());
    }
    let mut response = ui.add_enabled(view.is_enabled(), button);
    if let Some(reason) = &view.disabled {
        response = response.on_disabled_hover_text(reason.as_str());
    }
    response.clicked().then(|| {
        ui.close();
        Intent::new(view.id.clone())
    })
}

/// Draws `entries` into an open menu or context menu and returns the clicked command.
pub fn show_entries(ui: &mut Ui, entries: &[Entry]) -> Option<Intent> {
    let mut intent = None;
    for entry in entries {
        let clicked = match entry {
            Entry::Command(view) => command_button(ui, view),
            Entry::Submenu { label, entries } => ui
                .menu_button(label.as_str(), |ui| show_entries(ui, entries))
                .inner
                .flatten(),
            Entry::Separator => {
                ui.separator();
                None
            }
        };
        intent = intent.or(clicked);
    }
    intent
}

/// Draws a menu bar of `entries` and returns the clicked command.
pub fn show_menu_bar(ui: &mut Ui, entries: &[Entry]) -> Option<Intent> {
    egui::MenuBar::new()
        .ui(ui, |ui| show_entries(ui, entries))
        .inner
}

/// Draws a toolbar row and returns the clicked command. Buttons with an icon in `icons` show
/// the icon recolored with the text color token and expose the label as their accessible name
/// and tooltip; the others show the label.
pub fn show_toolbar(ui: &mut Ui, entries: &[ToolEntry], icons: Option<&Icons>) -> Option<Intent> {
    ui.horizontal(|ui| {
        let mut intent = None;
        let edge = ui.spacing().icon_width;
        for entry in entries {
            let ToolEntry::Command { view, icon } = entry else {
                ui.separator();
                continue;
            };
            let color = if view.is_enabled() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let image = icon
                .as_deref()
                .zip(icons)
                .and_then(|(name, icons)| icons.image(name, Vec2::splat(edge), color));
            let Some(image) = image else {
                intent = intent.or(command_button(ui, view));
                continue;
            };
            let enabled = view.is_enabled();
            let response = ui.add_enabled(enabled, Button::image(image));
            response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &view.label));
            let tip = match &view.binding {
                Some(binding) => format!("{} ({binding})", view.label),
                None => view.label.clone(),
            };
            let response = match &view.disabled {
                Some(reason) => response.on_disabled_hover_text(reason.as_str()),
                None => response.on_hover_text(tip),
            };
            if response.clicked() {
                intent = Some(Intent::new(view.id.clone()));
            }
        }
        intent
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::tests::{UndoDisabled, catalog, registry};
    use crate::keys::{Binding, Keymap};

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        r.map(|_| "ok").unwrap_or_else(|e| e.code())
    }

    fn cmd(id: &str) -> MenuItem {
        MenuItem::Command { command: id.into() }
    }

    #[test]
    fn menu_definitions_deserialize_and_validate() {
        let json = r#"{"items":[{"kind":"submenu","label":"demo-file","items":[
            {"kind":"command","command":"demo.file.save"},{"kind":"separator"}]}]}"#;
        let menu: Menu = serde_json::from_str(json).unwrap();
        assert_eq!(menu.items().len(), 1);
        let back: Menu = serde_json::from_str(&serde_json::to_string(&menu).unwrap()).unwrap();
        assert_eq!(back, menu);
        for bad in [
            r#"{"items":[{"kind":"command","command":"Not An Id"}]}"#,
            r#"{"items":[{"kind":"submenu","label":"bad label","items":[]}]}"#,
            r#"{"items":[{"kind":"command","command":"demo.file.save","extra":1}]}"#,
            r#"{"items":[],"other":1}"#,
        ] {
            assert!(serde_json::from_str::<Menu>(bad).is_err(), "{bad}");
        }
        let toolbar: Toolbar = serde_json::from_str(
            r#"{"items":[{"kind":"command","command":"demo.file.save","icon":"check"},{"kind":"separator"}]}"#,
        )
        .unwrap();
        assert_eq!(toolbar.items().len(), 2);
        assert!(
            serde_json::from_str::<Toolbar>(
                r#"{"items":[{"kind":"command","command":"demo.file.save","icon":"../x"}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn depth_and_size_are_bounded() {
        let mut item = cmd("demo.file.save");
        for _ in 0..MAX_DEPTH - 1 {
            item = MenuItem::Submenu {
                label: "demo-sub".into(),
                items: vec![item],
            };
        }
        assert_eq!(code(Menu::new(vec![item.clone()])), "ok");
        let deeper = MenuItem::Submenu {
            label: "demo-sub".into(),
            items: vec![item],
        };
        assert_eq!(code(Menu::new(vec![deeper])), "ui.menu.too_deep");
        let wide = vec![MenuItem::Separator; MAX_ITEMS + 1];
        assert_eq!(code(Menu::new(wide)), "ui.menu.too_large");
        assert_eq!(code(Menu::new(vec![MenuItem::Separator; MAX_ITEMS])), "ok");
        let toolbar = vec![ToolbarItem::Separator; MAX_ITEMS + 1];
        assert_eq!(code(Toolbar::new(toolbar)), "ui.menu.too_large");
    }

    #[test]
    fn resolution_shows_labels_bindings_and_disabled_reasons() {
        let registry = registry();
        let catalog = catalog();
        let (keymap, _) = Keymap::build(
            &[Binding {
                keys: "Ctrl+S".into(),
                command: "demo.file.save".into(),
            }],
            &registry,
        );
        let lookup = Lookup {
            registry: &registry,
            localize: &catalog,
            state: &UndoDisabled,
            keymap: Some(&keymap),
        };
        let menu = Menu::new(vec![MenuItem::Submenu {
            label: "demo-file".into(),
            items: vec![
                cmd("demo.file.save"),
                cmd("demo.gone.cmd"),
                MenuItem::Separator,
                cmd("demo.edit.undo"),
            ],
        }])
        .unwrap();
        let (entries, problems) = lookup.menu(&menu);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].code(), "ui.command.unknown");
        let Entry::Submenu { label, entries } = &entries[0] else {
            panic!("submenu expected")
        };
        assert_eq!(label, "demo-file");
        assert_eq!(entries.len(), 3);
        let Entry::Command(save) = &entries[0] else {
            panic!()
        };
        assert_eq!(
            (save.label.as_str(), save.binding.as_deref()),
            ("Save", Some("Ctrl+S"))
        );
        let Entry::Command(undo) = &entries[2] else {
            panic!()
        };
        assert_eq!(undo.disabled.as_deref(), Some("Nothing to undo in Plan"));
    }
}
