//! What widgets show about a command, resolved against the command registry, and the intents
//! they emit.
//!
//! Widgets never run commands: they return an [`Intent`] naming the command, and the
//! application invokes it through `arc-command`.

use arc_base::error::Value;
use arc_base::{Error, Result};
use arc_command::Registry;
use arc_command::spec::{CommandSpec, Disabled, Surface};

use crate::error::codes;
use crate::keys::Keymap;
use crate::text::{Localize, resolve};

/// A request to invoke a command, emitted by a widget.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Intent {
    /// The command identifier.
    pub command: String,
}

/// Whether a command can run now. The application answers from the command's enabled
/// predicate with the current document, selection and session.
pub trait CommandState {
    /// `Ok` when `spec` is enabled, or the reason it is disabled.
    fn enabled(&self, spec: &CommandSpec) -> std::result::Result<(), Disabled>;
}

/// Every command is enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllEnabled;

impl CommandState for AllEnabled {
    fn enabled(&self, _: &CommandSpec) -> std::result::Result<(), Disabled> {
        Ok(())
    }
}

/// A command as a widget shows it, with every text localized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandView {
    /// The command identifier.
    pub id: String,
    /// The localized label.
    pub label: String,
    /// The localized search keywords.
    pub keywords: Vec<String>,
    /// The user aliases.
    pub aliases: Vec<String>,
    /// The display text of the command's first key binding.
    pub binding: Option<String>,
    /// The localized reason when the command is disabled.
    pub disabled: Option<String>,
}

impl CommandView {
    /// True when the command can be invoked.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.disabled.is_none()
    }
}

/// Everything resolution reads.
#[derive(Clone, Copy)]
pub struct Lookup<'a> {
    /// The command registry.
    pub registry: &'a Registry,
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The enabled state.
    pub state: &'a dyn CommandState,
    /// The key bindings, when there are any.
    pub keymap: Option<&'a Keymap>,
}

impl Lookup<'_> {
    /// The view of command `id`. Fails with `ui.command.unknown` when it is not registered or
    /// not exposed on the UI surface.
    pub fn view(&self, id: &str) -> Result<CommandView> {
        let spec = self
            .registry
            .get(id)
            .filter(|spec| spec.exposure().contains(&Surface::Ui))
            .ok_or_else(|| Error::new(codes::COMMAND_UNKNOWN).with_arg("command", id))?;
        Ok(self.view_of(spec))
    }

    /// Views of every command exposed on the UI surface, in identifier order.
    #[must_use]
    pub fn all(&self) -> Vec<CommandView> {
        self.registry
            .iter()
            .filter(|spec| spec.exposure().contains(&Surface::Ui))
            .map(|spec| self.view_of(spec))
            .collect()
    }

    fn view_of(&self, spec: &CommandSpec) -> CommandView {
        let keywords = spec
            .keywords()
            .map(|id| resolve(self.localize, id, &[]))
            .map(|text| {
                text.split([',', ' ', '\t', '\n'])
                    .filter(|w| !w.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let disabled = self.state.enabled(spec).err().map(|reason| {
            let args: Vec<(&str, Value)> = reason
                .arguments
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            resolve(self.localize, reason.message_id, &args)
        });
        CommandView {
            id: spec.id().to_owned(),
            label: resolve(self.localize, spec.label(), &[]),
            keywords,
            aliases: self
                .registry
                .user_aliases()
                .filter(|(_, id)| *id == spec.id())
                .map(|(alias, _)| alias.to_owned())
                .collect(),
            binding: self.keymap.and_then(|k| k.display(spec.id())),
            disabled,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::text::tests::Map;
    use arc_command::spec::Permission;

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct NoParams {}

    /// A registry of `count` commands `demo.cmd.cN` labeled `demo-cN`, plus `demo.file.save`
    /// and `demo.edit.undo`, and `demo.hidden.cmd` exposed only to automation.
    pub fn registry() -> Registry {
        let mut r = Registry::new();
        for (id, label) in [
            ("demo.file.save", "demo-save"),
            ("demo.file.open", "demo-open"),
            ("demo.edit.undo", "demo-undo"),
        ] {
            r.register(
                CommandSpec::new(id, label, "demo-description", |_, _: NoParams| Ok(()))
                    .with_keywords("demo-keywords")
                    .with_permissions([Permission::Read]),
            )
            .unwrap();
        }
        r.register(
            CommandSpec::new(
                "demo.hidden.cmd",
                "demo-hidden",
                "demo-description",
                |_, _: NoParams| Ok(()),
            )
            .with_exposure([Surface::Automation]),
        )
        .unwrap();
        r.add_user_alias("s", "demo.file.save").unwrap();
        r
    }

    pub fn catalog() -> Map {
        Map::of(&[
            ("demo-save", "Save"),
            ("demo-open", "Open"),
            ("demo-undo", "Undo"),
            ("demo-keywords", "file, disk"),
            ("demo-nothing-to-undo", "Nothing to undo in { $doc }"),
        ])
    }

    /// Disables `demo.edit.undo`.
    pub struct UndoDisabled;
    impl CommandState for UndoDisabled {
        fn enabled(&self, spec: &CommandSpec) -> std::result::Result<(), Disabled> {
            if spec.id() == "demo.edit.undo" {
                Err(Disabled::new("demo-nothing-to-undo").with_arg("doc", "Plan"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn view_has_localized_label_keywords_alias_and_disabled_reason() {
        let registry = registry();
        let catalog = catalog();
        let lookup = Lookup {
            registry: &registry,
            localize: &catalog,
            state: &UndoDisabled,
            keymap: None,
        };
        let save = lookup.view("demo.file.save").unwrap();
        assert_eq!(save.label, "Save");
        assert_eq!(save.keywords, ["file", "disk"]);
        assert_eq!(save.aliases, ["s"]);
        assert!(save.is_enabled());
        let undo = lookup.view("demo.edit.undo").unwrap();
        assert_eq!(undo.disabled.as_deref(), Some("Nothing to undo in Plan"));
        for id in ["demo.hidden.cmd", "demo.nope.cmd"] {
            assert_eq!(lookup.view(id).unwrap_err().code(), "ui.command.unknown");
        }
        assert_eq!(lookup.all().len(), 3);
    }
}
