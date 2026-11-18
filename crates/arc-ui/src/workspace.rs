//! Workspaces (CORE-UI-002).
//!
//! A [`Workspace`] is a named dock layout with toolbars and tool sets, validated on
//! construction and deserialization. [`Workspaces`] holds the product's built-in workspaces,
//! the user's saved ones (a saved workspace with a built-in name replaces that built-in until it
//! is reset) and the active name. Export writes one workspace as versioned JSON text and import
//! reads it back with a size limit; the caller does the file input and output.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Error, Result};
use arc_command::spec::is_command_id;
use serde::{Deserialize, Serialize};

use crate::dock::DockLayout;
use crate::error::codes;
use crate::menu::Toolbar;

/// The longest workspace, toolbar or tool set name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The most toolbars in a workspace.
pub const MAX_TOOLBARS: usize = 32;
/// The most tool sets in a workspace.
pub const MAX_TOOL_SETS: usize = 64;
/// The most commands in a tool set.
pub const MAX_TOOLS: usize = 256;
/// The most saved workspaces, and the most built-in ones.
pub const MAX_WORKSPACES: usize = 64;
/// The largest import text, in bytes.
pub const MAX_IMPORT_BYTES: usize = 1024 * 1024;
/// The format name of an exported workspace.
pub const FORMAT: &str = "arc-workspace";
/// The version of the export format this crate writes and reads.
pub const VERSION: u32 = 1;

/// A toolbar with its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedToolbar {
    /// The name, unique in the workspace.
    pub name: String,
    /// The toolbar.
    pub toolbar: Toolbar,
}

/// A named set of tools (command identifiers) shown together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSet {
    /// The name, unique in the workspace.
    pub name: String,
    /// The command identifiers.
    pub commands: Vec<String>,
}

/// A validated workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WorkspaceRaw", into = "WorkspaceRaw")]
pub struct Workspace {
    name: String,
    layout: DockLayout,
    toolbars: Vec<NamedToolbar>,
    tool_sets: Vec<ToolSet>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRaw {
    name: String,
    layout: DockLayout,
    #[serde(default)]
    toolbars: Vec<NamedToolbar>,
    #[serde(default)]
    tool_sets: Vec<ToolSet>,
}

fn invalid(reason: &'static str) -> Error {
    Error::new(codes::WORKSPACE_INVALID).with_detail("reason", reason)
}

/// True when `name` has 1 to [`MAX_NAME_CHARS`] characters, no control characters and no
/// white space at either end.
#[must_use]
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !is_name(name) {
            return Err(invalid("name"));
        }
        if !seen.insert(name) {
            return Err(invalid("duplicate_name"));
        }
    }
    Ok(())
}

impl Workspace {
    /// A workspace.
    ///
    /// # Errors
    ///
    /// `ui.workspace.invalid` for a malformed or repeated name or a malformed command
    /// identifier; `ui.workspace.too_large` beyond [`MAX_TOOLBARS`], [`MAX_TOOL_SETS`] or
    /// [`MAX_TOOLS`].
    pub fn new(
        name: impl Into<String>,
        layout: DockLayout,
        toolbars: Vec<NamedToolbar>,
        tool_sets: Vec<ToolSet>,
    ) -> Result<Self> {
        let name = name.into();
        if !is_name(&name) {
            return Err(invalid("name"));
        }
        if toolbars.len() > MAX_TOOLBARS
            || tool_sets.len() > MAX_TOOL_SETS
            || tool_sets.iter().any(|s| s.commands.len() > MAX_TOOLS)
        {
            return Err(Error::new(codes::WORKSPACE_TOO_LARGE));
        }
        unique_names(toolbars.iter().map(|t| t.name.as_str()))?;
        unique_names(tool_sets.iter().map(|t| t.name.as_str()))?;
        if !tool_sets
            .iter()
            .flat_map(|s| &s.commands)
            .all(|c| is_command_id(c))
        {
            return Err(invalid("command"));
        }
        Ok(Self {
            name,
            layout,
            toolbars,
            tool_sets,
        })
    }

    /// The name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The dock layout.
    #[must_use]
    pub fn layout(&self) -> &DockLayout {
        &self.layout
    }

    /// The dock layout, for the dock widget; a layout is valid by construction.
    pub fn layout_mut(&mut self) -> &mut DockLayout {
        &mut self.layout
    }

    /// The toolbars.
    #[must_use]
    pub fn toolbars(&self) -> &[NamedToolbar] {
        &self.toolbars
    }

    /// The tool sets.
    #[must_use]
    pub fn tool_sets(&self) -> &[ToolSet] {
        &self.tool_sets
    }

    /// This workspace under another name.
    ///
    /// # Errors
    ///
    /// `ui.workspace.invalid` for a malformed name.
    pub fn renamed(mut self, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if !is_name(&name) {
            return Err(invalid("name"));
        }
        self.name = name;
        Ok(self)
    }
}

impl TryFrom<WorkspaceRaw> for Workspace {
    type Error = Error;
    fn try_from(raw: WorkspaceRaw) -> Result<Self> {
        Self::new(raw.name, raw.layout, raw.toolbars, raw.tool_sets)
    }
}

impl From<Workspace> for WorkspaceRaw {
    fn from(w: Workspace) -> Self {
        Self {
            name: w.name,
            layout: w.layout,
            toolbars: w.toolbars,
            tool_sets: w.tool_sets,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    format: String,
    version: u32,
    workspace: serde_json::Value,
}

/// Built-in and saved workspaces and the active one.
#[derive(Debug, Clone, PartialEq)]
pub struct Workspaces {
    builtin: Vec<Workspace>,
    saved: BTreeMap<String, Workspace>,
    active: String,
    /// The first built-in workspace, so there is always an active one.
    first: Workspace,
}

fn unknown(name: &str) -> Error {
    Error::new(codes::WORKSPACE_UNKNOWN).with_arg("name", name.to_owned())
}

impl Workspaces {
    /// The product's built-in workspaces; the first is active.
    ///
    /// # Errors
    ///
    /// `ui.workspace.invalid` for none or repeated names, `ui.workspace.too_large` beyond
    /// [`MAX_WORKSPACES`].
    pub fn new(builtin: Vec<Workspace>) -> Result<Self> {
        if builtin.len() > MAX_WORKSPACES {
            return Err(Error::new(codes::WORKSPACE_TOO_LARGE));
        }
        unique_names(builtin.iter().map(Workspace::name))?;
        let first = builtin
            .first()
            .ok_or_else(|| invalid("no_workspace"))?
            .clone();
        Ok(Self {
            active: first.name.clone(),
            builtin,
            saved: BTreeMap::new(),
            first,
        })
    }

    /// The workspace named `name`: the saved one, else the built-in one.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Workspace> {
        self.saved
            .get(name)
            .or_else(|| self.builtin.iter().find(|w| w.name == name))
    }

    /// The active workspace.
    #[must_use]
    pub fn active(&self) -> &Workspace {
        // The active name always names an existing workspace; the fallback is never taken.
        self.get(&self.active).unwrap_or(&self.first)
    }

    /// The names of every workspace, built-in ones first in their order, then saved ones.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let builtin = self.builtin.iter().map(Workspace::name);
        let own = self
            .saved
            .keys()
            .map(String::as_str)
            .filter(|n| !self.builtin.iter().any(|w| w.name == *n));
        builtin.chain(own).collect()
    }

    /// Makes `name` active.
    ///
    /// # Errors
    ///
    /// `ui.workspace.unknown`.
    pub fn switch(&mut self, name: &str) -> Result<()> {
        self.get(name).ok_or_else(|| unknown(name))?;
        name.clone_into(&mut self.active);
        Ok(())
    }

    /// Saves `workspace` under its name, replacing a saved one of that name.
    ///
    /// # Errors
    ///
    /// `ui.workspace.too_large` beyond [`MAX_WORKSPACES`] saved workspaces.
    pub fn save(&mut self, workspace: Workspace) -> Result<()> {
        if !self.saved.contains_key(&workspace.name) && self.saved.len() >= MAX_WORKSPACES {
            return Err(Error::new(codes::WORKSPACE_TOO_LARGE));
        }
        self.saved.insert(workspace.name.clone(), workspace);
        Ok(())
    }

    /// Drops the saved copy of the built-in workspace `name`, so the built-in one applies again.
    ///
    /// # Errors
    ///
    /// `ui.workspace.unknown` when no built-in workspace has that name.
    pub fn reset(&mut self, name: &str) -> Result<()> {
        if !self.builtin.iter().any(|w| w.name == name) {
            return Err(unknown(name));
        }
        self.saved.remove(name);
        Ok(())
    }

    /// The export text of workspace `name`.
    ///
    /// # Errors
    ///
    /// `ui.workspace.unknown`.
    pub fn export(&self, name: &str) -> Result<String> {
        let workspace = self.get(name).ok_or_else(|| unknown(name))?;
        let export = Export {
            format: FORMAT.to_owned(),
            version: VERSION,
            workspace: serde_json::to_value(workspace).map_err(|_| invalid("encode"))?,
        };
        serde_json::to_string_pretty(&export).map_err(|_| invalid("encode"))
    }

    /// Reads an export text. The caller saves the result to add it.
    ///
    /// # Errors
    ///
    /// `ui.workspace.too_large` beyond [`MAX_IMPORT_BYTES`]; `ui.workspace.version` for another
    /// format or a newer version; `ui.workspace.invalid` for text that does not parse, with the
    /// detail `message`; the validation errors of workspaces and layouts.
    pub fn import(text: &str) -> Result<Workspace> {
        if text.len() > MAX_IMPORT_BYTES {
            return Err(Error::new(codes::WORKSPACE_TOO_LARGE));
        }
        let parse = |e: serde_json::Error| invalid("parse").with_detail("message", e.to_string());
        let export: Export = serde_json::from_str(text).map_err(parse)?;
        if export.format != FORMAT || export.version != VERSION {
            return Err(
                Error::new(codes::WORKSPACE_VERSION).with_arg("version", i64::from(export.version))
            );
        }
        serde_json::from_value(export.workspace).map_err(parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Side, Target};
    use crate::menu::ToolbarItem;
    use proptest::prelude::*;

    fn workspace(name: &str, panels: &[&str]) -> Workspace {
        let mut layout = DockLayout::default();
        for p in panels {
            layout.add(p, &Target::Root { side: Side::Right }).unwrap();
        }
        let toolbar = Toolbar::new(vec![ToolbarItem::Command {
            command: "demo.file.save".into(),
            icon: None,
        }])
        .unwrap();
        Workspace::new(
            name,
            layout,
            vec![NamedToolbar {
                name: "main".into(),
                toolbar,
            }],
            vec![ToolSet {
                name: "draw".into(),
                commands: vec!["demo.draw.line".into()],
            }],
        )
        .unwrap()
    }

    #[test]
    fn save_switch_reset_export_import() {
        let mut all = Workspaces::new(vec![
            workspace("Modeling", &["tree"]),
            workspace("Drafting", &["sheets"]),
        ])
        .unwrap();
        assert_eq!(all.active().name(), "Modeling");
        let mut mine = all.active().clone();
        mine.layout_mut()
            .add("props", &Target::Root { side: Side::Left })
            .unwrap();
        all.save(mine.clone()).unwrap();
        assert_eq!(all.active(), &mine);
        all.save(mine.clone().renamed("Mine").unwrap()).unwrap();
        all.switch("Mine").unwrap();
        assert_eq!(all.names(), ["Modeling", "Drafting", "Mine"]);
        assert_eq!(
            all.switch("Nope").unwrap_err().code(),
            "ui.workspace.unknown"
        );
        assert_eq!(
            all.reset("Mine").unwrap_err().code(),
            "ui.workspace.unknown"
        );
        all.reset("Modeling").unwrap();
        assert_eq!(all.get("Modeling").unwrap().layout().panels(), ["tree"]);
        let text = all.export("Mine").unwrap();
        assert_eq!(
            Workspaces::import(&text).unwrap(),
            all.get("Mine").unwrap().clone()
        );
        let newer = text.replace("\"version\": 1", "\"version\": 2");
        assert_eq!(
            Workspaces::import(&newer).unwrap_err().code(),
            "ui.workspace.version"
        );
        assert_eq!(
            Workspaces::import("{").unwrap_err().code(),
            "ui.workspace.invalid"
        );
        assert_eq!(
            Workspaces::import(&" ".repeat(MAX_IMPORT_BYTES + 1))
                .unwrap_err()
                .code(),
            "ui.workspace.too_large"
        );
        let bad_layout = text.replace("\"tree\"", "\"bad id\"");
        assert!(Workspaces::import(&bad_layout).is_err());
        assert!(Workspaces::new(vec![]).is_err());
        assert!(Workspaces::new(vec![workspace("A", &[]), workspace("A", &[])]).is_err());
        assert!(Workspace::new(" padded", DockLayout::default(), vec![], vec![]).is_err());
    }

    proptest! {
        #[test]
        fn workspaces_round_trip(name in "[A-Za-z][A-Za-z0-9 ]{0,20}[a-z]", panels in proptest::collection::btree_set("[a-z]{1,5}", 0..8)) {
            let panels: Vec<&str> = panels.iter().map(String::as_str).collect();
            let w = workspace(&name, &panels);
            let all = Workspaces::new(vec![w.clone()]).unwrap();
            prop_assert_eq!(Workspaces::import(&all.export(&name).unwrap()).unwrap(), w);
        }

        #[test]
        fn hostile_imports_are_rejected_without_panic(
            name in "\\PC{0,80}",
            commands in proptest::collection::vec("[a-z._]{0,12}", 0..300),
            sets in 0usize..70,
            junk in "\\PC{0,200}",
        ) {
            let tool_sets: Vec<serde_json::Value> = (0..sets).map(|i| serde_json::json!({"name": format!("s{i}"), "commands": commands})).collect();
            let text = serde_json::json!({
                "format": FORMAT, "version": VERSION,
                "workspace": {"name": name, "layout": {"root": 0, "nodes": [{"kind": "tabs", "tabs": []}]}, "tool_sets": tool_sets}
            }).to_string();
            if let Ok(w) = Workspaces::import(&text) {
                prop_assert!(is_name(w.name()));
                prop_assert!(w.tool_sets().len() <= MAX_TOOL_SETS);
            }
            let _ = Workspaces::import(&junk);
        }
    }
}
