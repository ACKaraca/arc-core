//! The settings editor model (CORE-APP-011): every registered setting with its effective value
//! and source, search over keys and localized descriptions, and edits and resets per setting and
//! scope that go through the [`SettingsStore`] and, when given, the scope's settings file.

use std::path::Path;

use arc_base::Result;

use crate::locale::Localizer;
use crate::settings::{Issue, Scope, SettingValue, SettingsStore};
use crate::settings_file;

/// One line of the editor.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The setting key.
    pub key: &'static str,
    /// The effective value.
    pub value: SettingValue,
    /// The scope that supplied it.
    pub source: Scope,
    /// The highest scope that may supply a value.
    pub scope: Scope,
    /// The Fluent identifier of the description.
    pub message_id: &'static str,
    /// Stored values that resolution skipped, highest scope first.
    pub issues: Vec<Issue>,
    /// Whether the system scope locks the key; a locked key cannot be edited outside it.
    pub locked: bool,
}

/// Every registered setting, ordered by key.
#[must_use]
pub fn rows(store: &SettingsStore) -> Vec<Row> {
    store
        .registry()
        .iter()
        .filter_map(|setting| {
            // Every key comes from the registry, so resolution cannot fail.
            let effective = store.get(setting.key).ok()?;
            Some(Row {
                key: setting.key,
                value: effective.value,
                source: effective.source,
                scope: setting.scope,
                message_id: setting.message_id,
                issues: effective.issues,
                locked: store.is_locked(setting.key),
            })
        })
        .collect()
}

/// The rows whose key or localized description contains `query`, ignoring case; an empty or
/// blank query matches every row.
#[must_use]
pub fn search(store: &SettingsStore, localizer: &Localizer, query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    rows(store)
        .into_iter()
        .filter(|row| {
            row.key.contains(&query)
                || localizer
                    .format(row.message_id, &[])
                    .text
                    .to_lowercase()
                    .contains(&query)
        })
        .collect()
}

/// Writes the table of `scope` to `file`, when one is given.
fn persist(store: &SettingsStore, scope: Scope, file: Option<&Path>) -> Result<()> {
    match file {
        Some(file) => settings_file::save(file, &store.layer(scope).cloned().unwrap_or_default()),
        None => Ok(()),
    }
}

/// Sets `key` in `scope` and saves the scope's table to `file`.
///
/// # Errors
/// The errors of [`SettingsStore::set`], which change nothing, and
/// `app.settings_file.write_failed`, after which the value is set in memory and the file is
/// unchanged.
pub fn set(
    store: &mut SettingsStore,
    scope: Scope,
    key: &str,
    value: SettingValue,
    file: Option<&Path>,
) -> Result<()> {
    store.set(scope, key, value)?;
    persist(store, scope, file)
}

/// Removes the value of `key` in `scope`, so a lower scope or the default shows through, and
/// saves the scope's table to `file` when a value was removed. Returns whether one was.
///
/// # Errors
/// `app.settings.unknown_key` and `app.settings_file.write_failed`, as for [`set`].
pub fn reset(
    store: &mut SettingsStore,
    scope: Scope,
    key: &str,
    file: Option<&Path>,
) -> Result<bool> {
    let removed = store.reset(scope, key)?;
    if removed {
        persist(store, scope, file)?;
    }
    Ok(removed)
}
