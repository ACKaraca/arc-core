//! Typed settings and scope resolution (CORE-APP-010).
//!
//! A [`SettingsRegistry`] holds the registered [`Setting`]s. A [`SettingsStore`] holds one TOML
//! table per scope and resolves a key to its effective value and the scope that supplied it.
//! Keys are dotted; in a table, `ui.language` is the key `language` of the table `ui`.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Error, Result};
use toml::{Table, Value};

use crate::codes;

/// Where a value comes from, lowest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// The default registered with the setting.
    Default,
    /// The administrator file.
    System,
    /// The user's settings file.
    User,
    /// The workspace's settings.
    Workspace,
    /// The open document's settings.
    Document,
}

impl Scope {
    /// The scopes that hold values, highest precedence first.
    const LAYERS: [Self; 4] = [Self::Document, Self::Workspace, Self::User, Self::System];

    /// The lowercase name used in details and diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::System => "system",
            Self::User => "user",
            Self::Workspace => "workspace",
            Self::Document => "document",
        }
    }
}

/// The type of a setting.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingType {
    /// A truth value.
    Bool,
    /// An integer within an inclusive range.
    Int {
        /// The smallest accepted value.
        min: i64,
        /// The largest accepted value.
        max: i64,
    },
    /// A finite real number; an integer in a file is accepted and converted.
    Float,
    /// Any text.
    Text,
    /// A text equal to one of the listed choices.
    Choice(&'static [&'static str]),
}

/// A typed setting value.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    /// A truth value.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A finite real number.
    Float(f64),
    /// A text.
    Text(String),
}

impl SettingType {
    /// Whether `value` is a valid value of this type.
    #[must_use]
    pub fn accepts(&self, value: &SettingValue) -> bool {
        match (self, value) {
            (Self::Bool, SettingValue::Bool(_)) | (Self::Text, SettingValue::Text(_)) => true,
            (Self::Int { min, max }, SettingValue::Int(v)) => (min..=max).contains(&v),
            (Self::Float, SettingValue::Float(v)) => v.is_finite(),
            (Self::Choice(choices), SettingValue::Text(v)) => choices.contains(&v.as_str()),
            _ => false,
        }
    }

    /// Reads a TOML value as this type, or `None` when it does not fit.
    fn read(&self, value: &Value) -> Option<SettingValue> {
        let typed = match (self, value) {
            (Self::Bool, Value::Boolean(v)) => SettingValue::Bool(*v),
            (Self::Int { .. }, Value::Integer(v)) => SettingValue::Int(*v),
            (Self::Float, Value::Float(v)) => SettingValue::Float(*v),
            #[allow(clippy::cast_precision_loss)]
            // shortcut: integers beyond 2^53 round, accepted for settings
            (Self::Float, Value::Integer(v)) => SettingValue::Float(*v as f64),
            (Self::Text | Self::Choice(_), Value::String(v)) => SettingValue::Text(v.clone()),
            _ => return None,
        };
        self.accepts(&typed).then_some(typed)
    }
}

impl SettingValue {
    fn to_toml(&self) -> Value {
        match self {
            Self::Bool(v) => Value::Boolean(*v),
            Self::Int(v) => Value::Integer(*v),
            Self::Float(v) => Value::Float(*v),
            Self::Text(v) => Value::String(v.clone()),
        }
    }
}

/// A registered setting.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    /// The dotted key: one to eight segments of lowercase letters, digits and underscores, each
    /// starting with a letter.
    pub key: &'static str,
    /// The value type.
    pub ty: SettingType,
    /// The value when no scope supplies one; it must satisfy `ty`.
    pub default: SettingValue,
    /// The highest scope that may supply a value. A value in a higher scope is reported and
    /// skipped; the system scope is always allowed.
    pub scope: Scope,
    /// The Fluent message identifier of the description.
    pub message_id: &'static str,
}

/// Why a stored value was skipped during resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// The value does not satisfy the setting's type.
    TypeMismatch,
    /// The scope is above the highest scope the setting allows.
    ScopeNotAllowed,
    /// The key is locked by the system scope, so this scope's value is ignored.
    Locked,
}

/// A stored value that resolution skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The setting key.
    pub key: String,
    /// The scope that held the value.
    pub scope: Scope,
    /// Why it was skipped.
    pub kind: IssueKind,
}

impl Issue {
    /// The issue as an error, for notifications and logs.
    #[must_use]
    pub fn to_error(&self) -> Error {
        let code = match self.kind {
            IssueKind::TypeMismatch => codes::SETTINGS_TYPE_MISMATCH,
            IssueKind::ScopeNotAllowed => codes::SETTINGS_SCOPE_NOT_ALLOWED,
            IssueKind::Locked => codes::SETTINGS_LOCKED,
        };
        Error::new(code)
            .with_arg("key", self.key.as_str())
            .with_arg("scope", self.scope.as_str())
    }
}

/// The result of resolving one key.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The effective value.
    pub value: SettingValue,
    /// The scope that supplied it.
    pub source: Scope,
    /// Stored values in higher scopes that were skipped, highest scope first.
    pub issues: Vec<Issue>,
}

const MAX_SEGMENTS: usize = 8;

/// The top-level table of the system (managed) settings file that holds the lock list. Keys
/// under it are not settings.
pub const MANAGED_TABLE: &str = "managed";

fn key_is_valid(key: &str) -> bool {
    let segments: Vec<&str> = key.split('.').collect();
    let reserved = segments.first() == Some(&MANAGED_TABLE);
    segments.len() <= MAX_SEGMENTS
        && !reserved
        && segments.iter().all(|s| {
            s.starts_with(|c: char| c.is_ascii_lowercase())
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// The registered settings by key.
#[derive(Debug, Clone, Default)]
pub struct SettingsRegistry {
    settings: BTreeMap<&'static str, Setting>,
}

impl SettingsRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a setting.
    ///
    /// # Errors
    /// `app.settings.invalid_key`, `app.settings.duplicate_key` or
    /// `app.settings.default_mismatch`.
    pub fn register(&mut self, setting: Setting) -> Result<()> {
        let key = setting.key;
        if !key_is_valid(key) {
            return Err(Error::new(codes::SETTINGS_INVALID_KEY).with_arg("key", key));
        }
        if self.settings.contains_key(key) {
            return Err(Error::new(codes::SETTINGS_DUPLICATE_KEY).with_arg("key", key));
        }
        if !setting.ty.accepts(&setting.default) {
            return Err(Error::new(codes::SETTINGS_DEFAULT_MISMATCH).with_arg("key", key));
        }
        self.settings.insert(key, setting);
        Ok(())
    }

    /// The setting registered under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.get(key)
    }

    /// Every setting, ordered by key.
    pub fn iter(&self) -> impl Iterator<Item = &Setting> {
        self.settings.values()
    }

    /// The settings `arc-app` itself registers: language, logging and recent files.
    #[must_use]
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        let all = [
            Setting {
                key: "ui.language",
                ty: SettingType::Text,
                default: SettingValue::Text("system".into()),
                scope: Scope::User,
                message_id: "app-setting-ui-language",
            },
            Setting {
                key: "log.level",
                ty: SettingType::Choice(&["error", "warn", "info", "debug", "trace"]),
                default: SettingValue::Text("info".into()),
                scope: Scope::User,
                message_id: "app-setting-log-level",
            },
            Setting {
                key: "log.max_file_bytes",
                ty: SettingType::Int {
                    min: 4096,
                    max: 1 << 30,
                },
                default: SettingValue::Int(5 << 20),
                scope: Scope::User,
                message_id: "app-setting-log-max-file-bytes",
            },
            Setting {
                key: "log.max_files",
                ty: SettingType::Int { min: 1, max: 100 },
                default: SettingValue::Int(5),
                scope: Scope::User,
                message_id: "app-setting-log-max-files",
            },
            Setting {
                key: "recent.max_entries",
                ty: SettingType::Int { min: 1, max: 200 },
                default: SettingValue::Int(20),
                scope: Scope::User,
                message_id: "app-setting-recent-max-entries",
            },
        ];
        for setting in all {
            // The table above is valid by construction; a failure would only drop one entry.
            let _ = registry.register(setting);
        }
        registry
    }
}

/// Walks a dotted key through nested tables.
fn lookup<'a>(table: &'a Table, key: &str) -> Option<&'a Value> {
    let mut parts = key.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

/// One table per value-holding scope, resolved against a registry.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    registry: SettingsRegistry,
    layers: BTreeMap<Scope, Table>,
    locked: BTreeSet<&'static str>,
}

impl SettingsStore {
    /// A store without values over `registry`.
    #[must_use]
    pub fn new(registry: SettingsRegistry) -> Self {
        Self {
            registry,
            layers: BTreeMap::new(),
            locked: BTreeSet::new(),
        }
    }

    /// Locks the registered keys in `keys` (managed settings, CORE-APP-013): their value comes
    /// from the system scope only, and writes below that scope are refused. Keys the registry
    /// does not know are not locked; they are returned, in order, so the caller can log them.
    pub fn lock<'k>(&mut self, keys: impl IntoIterator<Item = &'k str>) -> Vec<String> {
        let mut unknown = Vec::new();
        for key in keys {
            match self.registry.get(key) {
                Some(setting) => {
                    self.locked.insert(setting.key);
                }
                None => unknown.push(key.to_owned()),
            }
        }
        unknown
    }

    /// Whether `key` is locked by the system scope.
    #[must_use]
    pub fn is_locked(&self, key: &str) -> bool {
        self.locked.contains(key)
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &SettingsRegistry {
        &self.registry
    }

    /// Replaces the table of a scope, typically a loaded settings file. The table may hold keys
    /// the registry does not know and values of the wrong type; resolution skips the latter.
    ///
    /// # Errors
    /// `app.settings.scope_not_allowed` for [`Scope::Default`], which holds no table.
    pub fn set_layer(&mut self, scope: Scope, table: Table) -> Result<()> {
        if scope == Scope::Default {
            return Err(Error::new(codes::SETTINGS_SCOPE_NOT_ALLOWED).with_arg("scope", "default"));
        }
        self.layers.insert(scope, table);
        Ok(())
    }

    /// The table of a scope, for saving.
    #[must_use]
    pub fn layer(&self, scope: Scope) -> Option<&Table> {
        self.layers.get(&scope)
    }

    /// Resolves `key`: the highest scope holding a valid value wins, else the default.
    ///
    /// # Errors
    /// `app.settings.unknown_key`.
    pub fn get(&self, key: &str) -> Result<Effective> {
        let setting = self
            .registry
            .get(key)
            .ok_or_else(|| Error::new(codes::SETTINGS_UNKNOWN_KEY).with_arg("key", key))?;
        let mut issues = Vec::new();
        for scope in Scope::LAYERS {
            let Some(raw) = self.layers.get(&scope).and_then(|t| lookup(t, key)) else {
                continue;
            };
            let kind = if scope != Scope::System && self.locked.contains(setting.key) {
                IssueKind::Locked
            } else if scope > setting.scope && scope != Scope::System {
                IssueKind::ScopeNotAllowed
            } else if let Some(value) = setting.ty.read(raw) {
                return Ok(Effective {
                    value,
                    source: scope,
                    issues,
                });
            } else {
                IssueKind::TypeMismatch
            };
            issues.push(Issue {
                key: key.to_owned(),
                scope,
                kind,
            });
        }
        Ok(Effective {
            value: setting.default.clone(),
            source: Scope::Default,
            issues,
        })
    }

    /// Sets a value in a scope.
    ///
    /// # Errors
    /// `app.settings.unknown_key`, `app.settings.scope_not_allowed`,
    /// `app.settings.type_mismatch` or `app.settings.path_conflict`.
    pub fn set(&mut self, scope: Scope, key: &str, value: SettingValue) -> Result<()> {
        let setting = self
            .registry
            .get(key)
            .ok_or_else(|| Error::new(codes::SETTINGS_UNKNOWN_KEY).with_arg("key", key))?;
        if scope == Scope::Default || (scope > setting.scope && scope != Scope::System) {
            return Err(Error::new(codes::SETTINGS_SCOPE_NOT_ALLOWED)
                .with_arg("key", key)
                .with_arg("scope", scope.as_str()));
        }
        if scope != Scope::System && self.locked.contains(setting.key) {
            return Err(locked_error(key, scope));
        }
        if !setting.ty.accepts(&value) {
            return Err(Error::new(codes::SETTINGS_TYPE_MISMATCH).with_arg("key", key));
        }
        let conflict = || Error::new(codes::SETTINGS_PATH_CONFLICT).with_arg("key", key);
        let mut parts: Vec<&str> = key.split('.').collect();
        let leaf = parts.pop().ok_or_else(conflict)?;
        let mut table = self.layers.entry(scope).or_default();
        for part in parts {
            let next = table
                .entry(part.to_owned())
                .or_insert_with(|| Value::Table(Table::new()));
            table = next.as_table_mut().ok_or_else(conflict)?;
        }
        if table.get(leaf).is_some_and(Value::is_table) {
            return Err(conflict());
        }
        table.insert(leaf.to_owned(), value.to_toml());
        Ok(())
    }

    /// Removes the value of `key` from a scope, so lower scopes show through. Returns whether a
    /// value was removed. Parent tables left empty are removed too.
    ///
    /// # Errors
    /// `app.settings.unknown_key` and `app.settings.locked` (a locked key outside the system
    /// scope).
    pub fn reset(&mut self, scope: Scope, key: &str) -> Result<bool> {
        let Some(setting) = self.registry.get(key) else {
            return Err(Error::new(codes::SETTINGS_UNKNOWN_KEY).with_arg("key", key));
        };
        if scope != Scope::System && self.locked.contains(setting.key) {
            return Err(locked_error(key, scope));
        }
        let Some(table) = self.layers.get_mut(&scope) else {
            return Ok(false);
        };
        Ok(remove(table, key))
    }
}

fn locked_error(key: &str, scope: Scope) -> Error {
    Error::new(codes::SETTINGS_LOCKED)
        .with_arg("key", key)
        .with_arg("scope", scope.as_str())
}

fn remove(table: &mut Table, key: &str) -> bool {
    match key.split_once('.') {
        None => table.remove(key).is_some(),
        Some((head, rest)) => {
            let Some(child) = table.get_mut(head).and_then(Value::as_table_mut) else {
                return false;
            };
            let removed = remove(child, rest);
            if removed && child.is_empty() {
                table.remove(head);
            }
            removed
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn setting(key: &'static str, ty: SettingType, default: SettingValue, scope: Scope) -> Setting {
        Setting {
            key,
            ty,
            default,
            scope,
            message_id: "x",
        }
    }

    fn registry() -> SettingsRegistry {
        let mut r = SettingsRegistry::new();
        r.register(setting(
            "a.flag",
            SettingType::Bool,
            SettingValue::Bool(false),
            Scope::Document,
        ))
        .unwrap();
        r.register(setting(
            "a.size",
            SettingType::Int { min: 1, max: 10 },
            SettingValue::Int(5),
            Scope::User,
        ))
        .unwrap();
        r
    }

    fn table(text: &str) -> Table {
        text.parse().unwrap()
    }

    #[test]
    fn registration_rejects_bad_keys_duplicates_and_defaults() {
        let mut r = registry();
        let bad = |key| {
            setting(
                key,
                SettingType::Bool,
                SettingValue::Bool(true),
                Scope::User,
            )
        };
        for key in ["", "A", "a..b", "a.1", "a-b", "a.b.c.d.e.f.g.h.i"] {
            assert_eq!(
                r.register(bad(key)).unwrap_err().code(),
                "app.settings.invalid_key",
                "{key}"
            );
        }
        assert_eq!(
            r.register(bad("a.flag")).unwrap_err().code(),
            "app.settings.duplicate_key"
        );
        let mismatch = setting("z", SettingType::Bool, SettingValue::Int(1), Scope::User);
        assert_eq!(
            r.register(mismatch).unwrap_err().code(),
            "app.settings.default_mismatch"
        );
        let nan = setting(
            "y",
            SettingType::Float,
            SettingValue::Float(f64::NAN),
            Scope::User,
        );
        assert_eq!(
            r.register(nan).unwrap_err().code(),
            "app.settings.default_mismatch"
        );
    }

    #[test]
    fn builtin_registry_is_complete() {
        assert_eq!(SettingsRegistry::builtin().iter().count(), 5);
    }

    #[test]
    fn higher_scope_wins_and_source_is_reported() {
        let mut s = SettingsStore::new(registry());
        assert_eq!(s.get("a.size").unwrap().source, Scope::Default);
        s.set_layer(Scope::System, table("[a]\nsize = 2")).unwrap();
        s.set_layer(Scope::User, table("[a]\nsize = 3")).unwrap();
        let e = s.get("a.size").unwrap();
        assert_eq!((e.value, e.source), (SettingValue::Int(3), Scope::User));
        assert!(s.reset(Scope::User, "a.size").unwrap());
        assert_eq!(s.get("a.size").unwrap().source, Scope::System);
    }

    #[test]
    fn mismatch_and_disallowed_scope_are_reported_and_skipped() {
        let mut s = SettingsStore::new(registry());
        s.set_layer(Scope::System, table("[a]\nsize = 2")).unwrap();
        s.set_layer(Scope::User, table("[a]\nsize = 99")).unwrap(); // out of range
        s.set_layer(Scope::Workspace, table("[a]\nsize = 4"))
            .unwrap(); // scope above User
        s.set_layer(Scope::Document, table("[a]\nsize = \"x\""))
            .unwrap();
        let e = s.get("a.size").unwrap();
        assert_eq!((e.value, e.source), (SettingValue::Int(2), Scope::System));
        let seen: Vec<_> = e.issues.iter().map(|i| (i.scope, i.kind)).collect();
        assert_eq!(
            seen,
            vec![
                (Scope::Document, IssueKind::ScopeNotAllowed),
                (Scope::Workspace, IssueKind::ScopeNotAllowed),
                (Scope::User, IssueKind::TypeMismatch),
            ]
        );
        assert_eq!(e.issues[2].to_error().code(), "app.settings.type_mismatch");
    }

    #[test]
    fn set_validates_and_keeps_unknown_keys() {
        let mut s = SettingsStore::new(registry());
        s.set_layer(Scope::User, table("other = 1\n[a]\nextra = true"))
            .unwrap();
        s.set(Scope::User, "a.size", SettingValue::Int(7)).unwrap();
        let t = s.layer(Scope::User).unwrap();
        assert_eq!(t["other"], Value::Integer(1));
        assert_eq!(t["a"]["extra"], Value::Boolean(true));
        let code = |r: Result<()>| r.unwrap_err().code();
        assert_eq!(
            code(s.set(Scope::User, "a.size", SettingValue::Int(11))),
            "app.settings.type_mismatch"
        );
        assert_eq!(
            code(s.set(Scope::Workspace, "a.size", SettingValue::Int(1))),
            "app.settings.scope_not_allowed"
        );
        assert_eq!(
            code(s.set(Scope::Default, "a.size", SettingValue::Int(1))),
            "app.settings.scope_not_allowed"
        );
        assert_eq!(
            code(s.set(Scope::User, "nope", SettingValue::Int(1))),
            "app.settings.unknown_key"
        );
        s.set_layer(Scope::System, table("a = 5")).unwrap();
        assert_eq!(
            code(s.set(Scope::System, "a.size", SettingValue::Int(1))),
            "app.settings.path_conflict"
        );
        assert_eq!(s.get("zzz").unwrap_err().code(), "app.settings.unknown_key");
    }

    #[test]
    fn reset_prunes_only_emptied_parents() {
        let mut s = SettingsStore::new(registry());
        s.set(Scope::User, "a.size", SettingValue::Int(7)).unwrap();
        assert!(s.reset(Scope::User, "a.size").unwrap());
        assert!(s.layer(Scope::User).unwrap().is_empty());
        assert!(!s.reset(Scope::User, "a.size").unwrap());
    }

    #[test]
    fn float_accepts_integers_but_not_non_finite() {
        let ty = SettingType::Float;
        assert_eq!(ty.read(&Value::Integer(2)), Some(SettingValue::Float(2.0)));
        assert_eq!(ty.read(&Value::Float(f64::INFINITY)), None);
    }

    #[derive(Debug, Clone)]
    enum Raw {
        Bool(bool),
        Int(i64),
        Text(String),
    }

    fn raw() -> impl Strategy<Value = Option<Raw>> {
        prop_oneof![
            Just(None),
            any::<bool>().prop_map(|b| Some(Raw::Bool(b))),
            any::<i64>().prop_map(|i| Some(Raw::Int(i))),
            (-3i64..14).prop_map(|i| Some(Raw::Int(i))),
            prop_oneof![Just("red"), Just("blue"), Just("other")]
                .prop_map(|s| Some(Raw::Text(s.to_owned()))),
        ]
    }

    // The reference model: the first of document, workspace, user, system whose raw value fits
    // the type and whose scope the setting allows, else the default.
    fn model_fits(ty: u8, raw: &Raw) -> Option<SettingValue> {
        match (ty, raw) {
            (0, Raw::Bool(b)) => Some(SettingValue::Bool(*b)),
            (1, Raw::Int(i)) if (1..=10).contains(i) => Some(SettingValue::Int(*i)),
            (2, Raw::Text(t)) if t == "red" || t == "blue" => Some(SettingValue::Text(t.clone())),
            (3, Raw::Text(t)) => Some(SettingValue::Text(t.clone())),
            _ => None,
        }
    }

    proptest! {
        #[test]
        fn resolution_matches_the_reference_model(
            ty in 0u8..4,
            declared in 0usize..3,
            raws in proptest::collection::vec(raw(), 4),
        ) {
            let (st, default) = match ty {
                0 => (SettingType::Bool, SettingValue::Bool(false)),
                1 => (SettingType::Int { min: 1, max: 10 }, SettingValue::Int(5)),
                2 => (SettingType::Choice(&["red", "blue"]), SettingValue::Text("red".into())),
                _ => (SettingType::Text, SettingValue::Text("d".into())),
            };
            let declared = [Scope::User, Scope::Workspace, Scope::Document][declared];
            let mut reg = SettingsRegistry::new();
            reg.register(setting("k.v", st, default.clone(), declared)).unwrap();
            let mut store = SettingsStore::new(reg);
            let scopes = [Scope::System, Scope::User, Scope::Workspace, Scope::Document];
            for (scope, raw) in scopes.iter().zip(&raws) {
                let value = match raw {
                    None => continue,
                    Some(Raw::Bool(b)) => Value::Boolean(*b),
                    Some(Raw::Int(i)) => Value::Integer(*i),
                    Some(Raw::Text(t)) => Value::String(t.clone()),
                };
                let mut inner = Table::new();
                inner.insert("v".into(), value);
                let mut t = Table::new();
                t.insert("k".into(), Value::Table(inner));
                store.set_layer(*scope, t).unwrap();
            }
            let mut expected = (default, Scope::Default);
            let mut skipped = Vec::new();
            for (scope, raw) in scopes.iter().zip(&raws).rev() {
                let Some(raw) = raw else { continue };
                if *scope > declared && *scope != Scope::System {
                    skipped.push(*scope);
                } else if let Some(v) = model_fits(ty, raw) {
                    expected = (v, *scope);
                    break;
                } else {
                    skipped.push(*scope);
                }
            }
            let got = store.get("k.v").unwrap();
            prop_assert_eq!((got.value, got.source), expected);
            prop_assert_eq!(got.issues.iter().map(|i| i.scope).collect::<Vec<_>>(), skipped);
        }
    }
}
