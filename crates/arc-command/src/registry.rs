//! The registry, aliases and name resolution (CORE-CMD-009) and the command catalog
//! (CORE-CMD-010).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use arc_base::{Error, Result};
use arc_doc::{Diagnostic, Origin, Severity};
use serde_json::{Value, json};

use crate::error::codes;
use crate::spec::{CommandSpec, is_command_id};

/// The format name of the command catalog.
pub const CATALOG_FORMAT: &str = "arc-command-catalog";

/// The version of the catalog format; it changes when a field changes meaning or disappears.
pub const CATALOG_VERSION: u32 = 1;

/// The longest user alias, in bytes.
pub const MAX_ALIAS_BYTES: usize = 64;

/// How a typed name was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The command identifier.
    Identifier,
    /// A deprecated alias.
    DeprecatedAlias,
    /// A user alias.
    UserAlias,
    /// A localized command name.
    LocalizedName,
    /// The name resolver of the product ([`NameResolver`]).
    Product,
}

/// Flags a name resolver sets for the command a typed name resolves to. The flags reach the
/// command's [`Context`](crate::Context) and their meaning is the command's; this crate only
/// carries them. The default is no flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NameFlags {
    /// The command must not open a dialog: it uses its defaults or fails.
    pub no_dialogs: bool,
    /// The command uses its built-in definition, not a user redefinition.
    pub builtin_only: bool,
}

/// What a name resolver returns for a name it owns: the identifier of the command and the flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMatch {
    /// The command identifier. It must name a registered command; otherwise the resolution fails
    /// with `cmd.unknown`.
    pub command: String,
    /// The flags for the command's context.
    pub flags: NameFlags,
}

/// A product's name resolver: maps a typed name (surrounding whitespace removed, case as typed)
/// to a [`NameMatch`], or `None` when the name is not the product's. The registry consults it
/// before the built-in rules, on every surface that resolves names through the registry.
#[derive(Clone)]
pub struct NameResolver(Arc<NameFn>);

/// The function a [`NameResolver`] wraps.
type NameFn = dyn Fn(&str) -> Option<NameMatch> + Send + Sync;

impl NameResolver {
    /// A resolver from `resolve`.
    pub fn new(resolve: impl Fn(&str) -> Option<NameMatch> + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolve))
    }
}

impl fmt::Debug for NameResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NameResolver").finish_non_exhaustive()
    }
}

/// A resolved command name.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The command.
    pub spec: Arc<CommandSpec>,
    /// How the name was resolved.
    pub via: Via,
    /// The flags the name resolver set; the default for every other resolution.
    pub flags: NameFlags,
}

impl Resolved {
    /// The name to invoke the command with, for `typed`, the name the caller resolved: the typed
    /// name when a product's resolver matched it (the registry must see it again to set the same
    /// flags), else the command identifier, as before.
    #[must_use]
    pub fn call_name<'a>(&'a self, typed: &'a str) -> &'a str {
        if self.via == Via::Product {
            typed.trim()
        } else {
            self.spec.id()
        }
    }

    /// The `cmd.alias.deprecated` warning when the name was a deprecated alias.
    #[must_use]
    pub fn deprecation(&self, name: &str) -> Option<Diagnostic> {
        (self.via == Via::DeprecatedAlias).then(|| {
            Diagnostic::new(
                Severity::Warning,
                Error::new(codes::ALIAS_DEPRECATED)
                    .with_arg("alias", name)
                    .with_arg("command", self.spec.id()),
                Vec::new(),
                Origin::Executor,
            )
        })
    }
}

/// The commands of a process with their aliases. Iteration is in identifier order.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    commands: BTreeMap<String, Arc<CommandSpec>>,
    deprecated: BTreeMap<String, String>,
    user: BTreeMap<String, String>,
    names: Option<NameResolver>,
}

fn duplicate(name: &str) -> Error {
    Error::new(codes::DUPLICATE).with_arg("name", name)
}

/// True when `alias` is a user alias: 1 to [`MAX_ALIAS_BYTES`] bytes of lowercase ASCII
/// letters, digits, `_`, `.` and `-`, starting with a letter or digit.
fn is_user_alias(alias: &str) -> bool {
    alias.len() <= MAX_ALIAS_BYTES
        && alias.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && alias
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn taken(&self, name: &str) -> bool {
        self.commands.contains_key(name)
            || self.deprecated.contains_key(name)
            || self.user.contains_key(name)
    }

    /// Registers `spec` after [`CommandSpec::check`]. Fails, changing nothing, with
    /// `cmd.spec.invalid`, or `cmd.duplicate` when its identifier is a registered identifier or
    /// alias. Whether a duplicate stops start-up is the application's decision.
    pub fn register(&mut self, spec: CommandSpec) -> Result<()> {
        spec.check()?;
        if self.taken(spec.id()) {
            return Err(duplicate(spec.id()));
        }
        self.commands.insert(spec.id().to_owned(), Arc::new(spec));
        Ok(())
    }

    /// Adds the deprecated alias `alias`, a former identifier, of the command `id`. Fails with
    /// `cmd.alias.invalid` when `alias` is not an identifier, `cmd.unknown` when `id` is not
    /// registered, or `cmd.duplicate` when `alias` is taken.
    pub fn add_deprecated_alias(&mut self, alias: &str, id: &str) -> Result<()> {
        if !is_command_id(alias) {
            return Err(Error::new(codes::ALIAS_INVALID).with_arg("alias", alias));
        }
        self.add_alias(alias, id, true)
    }

    /// Adds the user alias `alias` of the command `id`. Fails with `cmd.alias.invalid`,
    /// `cmd.unknown`, or `cmd.duplicate` when `alias` is taken; a user alias never shadows an
    /// identifier or another alias.
    pub fn add_user_alias(&mut self, alias: &str, id: &str) -> Result<()> {
        if !is_user_alias(alias) {
            return Err(Error::new(codes::ALIAS_INVALID).with_arg("alias", alias));
        }
        self.add_alias(alias, id, false)
    }

    /// True when the product's name resolver owns `text`.
    fn owned_by_product(&self, text: &str) -> bool {
        self.names.as_ref().is_some_and(|r| (r.0)(text).is_some())
    }

    fn add_alias(&mut self, alias: &str, id: &str, deprecated: bool) -> Result<()> {
        if !self.commands.contains_key(id) {
            return Err(Error::new(codes::UNKNOWN).with_arg("name", id));
        }
        if self.taken(alias) || self.owned_by_product(alias) {
            return Err(duplicate(alias));
        }
        let table = if deprecated {
            &mut self.deprecated
        } else {
            &mut self.user
        };
        table.insert(alias.to_owned(), id.to_owned());
        Ok(())
    }

    /// Removes the user alias `alias`; fails with `cmd.alias.unknown` when there is none.
    pub fn remove_user_alias(&mut self, alias: &str) -> Result<()> {
        self.user
            .remove(alias)
            .map(drop)
            .ok_or_else(|| Error::new(codes::ALIAS_UNKNOWN).with_arg("alias", alias))
    }

    /// Sets the product's name resolver, replacing any earlier one, so every surface that resolves
    /// names through this registry sees its matches: the command line, the protocol, macros and
    /// the headless runner. Fails with `cmd.duplicate`, changing nothing, when the resolver owns the
    /// text of a deprecated or user alias that is already registered, since the alias would be dead.
    pub fn set_name_resolver(&mut self, resolver: NameResolver) -> Result<()> {
        let aliases = self.deprecated.keys().chain(self.user.keys());
        if let Some(alias) = aliases.clone().find(|a| (resolver.0)(a).is_some()) {
            return Err(duplicate(alias));
        }
        self.names = Some(resolver);
        Ok(())
    }

    /// The command `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<CommandSpec>> {
        self.commands.get(id)
    }

    /// Every command, in identifier order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<CommandSpec>> + '_ {
        self.commands.values()
    }

    /// The user aliases and their commands, in alias order.
    pub fn user_aliases(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.user.iter().map(|(a, id)| (a.as_str(), id.as_str()))
    }

    /// Resolves a typed name. Surrounding whitespace is ignored and ASCII letters are lowercased.
    /// The order is:
    ///
    /// 1. a registered command identifier, which the product's resolver can never shadow;
    /// 2. the product's name resolver ([`Registry::set_name_resolver`]), on the name with
    ///    surrounding whitespace removed; a match resolves to its command with its flags, and a
    ///    command it names that is not registered fails with `cmd.unknown`;
    /// 3. a deprecated alias, then a user alias;
    /// 4. the localized names: the caller passes a map from identifier to localized name taken
    ///    from the application's active catalog, compared case-insensitively.
    ///
    /// Fails with `cmd.unknown`, or `cmd.name.ambiguous` when several commands have the localized
    /// name. The transparent prefix of the command line is taken off the line before a name is
    /// resolved, so it never reaches this order.
    ///
    /// Resolution does not check exposure: the invoker refuses a command that is not exposed on
    /// the caller's surface with `cmd.not_exposed`, for a name resolved by a product's resolver as
    /// for any other name.
    pub fn resolve(&self, name: &str, localized: &BTreeMap<String, String>) -> Result<Resolved> {
        let trimmed = name.trim();
        let key = trimmed.to_ascii_lowercase();
        let found = |id: &str, via| {
            self.commands.get(id).map(|spec| Resolved {
                spec: Arc::clone(spec),
                via,
                flags: NameFlags::default(),
            })
        };
        if let Some(resolved) = found(&key, Via::Identifier) {
            return Ok(resolved);
        }
        if let Some(matched) = self.names.as_ref().and_then(|r| (r.0)(trimmed)) {
            let spec = self
                .commands
                .get(&matched.command)
                .ok_or_else(|| Error::new(codes::UNKNOWN).with_arg("name", trimmed))?;
            return Ok(Resolved {
                spec: Arc::clone(spec),
                via: Via::Product,
                flags: matched.flags,
            });
        }
        let direct = self
            .deprecated
            .get(&key)
            .and_then(|id| found(id, Via::DeprecatedAlias))
            .or_else(|| self.user.get(&key).and_then(|id| found(id, Via::UserAlias)));
        if let Some(resolved) = direct {
            return Ok(resolved);
        }
        let wanted = trimmed.to_lowercase();
        let mut matches = localized
            .iter()
            .filter(|(_, localized)| localized.trim().to_lowercase() == wanted)
            .filter_map(|(id, _)| found(id, Via::LocalizedName));
        match (matches.next(), matches.next()) {
            (Some(resolved), None) => Ok(resolved),
            (Some(_), Some(_)) => Err(Error::new(codes::NAME_AMBIGUOUS).with_arg("name", trimmed)),
            (None, _) => Err(Error::new(codes::UNKNOWN).with_arg("name", trimmed)),
        }
    }

    /// The command catalog: every command exposed on at least one surface, in identifier order,
    /// with its labels, schemas, examples, permission classes, exposure, undo behavior,
    /// transparency, prompt machine flag and deprecated aliases. Objects have sorted keys, so
    /// the same registry always gives the same document.
    #[must_use]
    pub fn catalog(&self) -> Value {
        let commands: Vec<Value> = self
            .iter()
            .filter(|spec| !spec.exposure().is_empty())
            .map(|spec| self.entry(spec))
            .collect();
        json!({
            "format": CATALOG_FORMAT,
            "version": CATALOG_VERSION,
            "commands": commands,
        })
    }

    /// The catalog entry of `spec`.
    pub(crate) fn entry(&self, spec: &CommandSpec) -> Value {
        let aliases: Vec<&String> = self
            .deprecated
            .iter()
            .filter(|(_, id)| id.as_str() == spec.id())
            .map(|(alias, _)| alias)
            .collect();
        json!({
            "id": spec.id(),
            "label": spec.label(),
            "description": spec.description(),
            "category": spec.category(),
            "keywords": spec.keywords(),
            "params": spec.params_schema(),
            "result": spec.result_schema(),
            "examples": spec.examples(),
            "permissions": spec.permissions(),
            "exposure": spec.exposure(),
            "undo": spec.undo(),
            "coalesce": spec.coalesce(),
            "transparent": spec.is_transparent(),
            "prompt": spec.prompt().is_some(),
            "deprecated_aliases": aliases,
        })
    }

    /// The catalog as pretty-printed JSON text.
    #[must_use]
    pub fn catalog_text(&self) -> String {
        serde_json::to_string_pretty(&self.catalog()).unwrap_or_default()
    }
}
