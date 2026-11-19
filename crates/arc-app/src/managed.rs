//! Managed settings (CORE-APP-013): the administrator's system-scope settings file.
//!
//! The file is a TOML table like a user settings file. Its values are the system scope. A
//! top-level table `managed` holds the lock list, `managed.locked`, an array of setting keys:
//! a locked key takes its value from the system scope only, and a write to it outside the
//! system scope is refused with `app.settings.locked`. A locked key with no system value takes
//! the default. Entries that are not registered keys are ignored and logged.
//!
//! The file is read, never written or renamed: it lives in a directory the user does not own. A
//! missing file applies nothing; a file that cannot be used is reported as an error and nothing
//! from it is applied.

use std::fs;
use std::path::Path;

use arc_base::{Error, Result};
use toml::{Table, Value};

use crate::codes;
use crate::settings::{MANAGED_TABLE, Scope, SettingsStore};
use crate::settings_file::MAX_FILE_BYTES;

/// The key of the lock list inside [`MANAGED_TABLE`].
pub const LOCKED: &str = "locked";

/// What [`apply`] took from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    /// The keys locked by the file, in file order.
    pub locked: Vec<String>,
    /// Lock-list entries that were ignored: unknown keys, non-text entries and a list that is
    /// not an array.
    pub ignored: Vec<String>,
}

/// Reads the managed settings table. A missing file yields an empty table.
///
/// # Errors
/// `app.managed.read_failed` when the file exists but cannot be read, and `app.managed.invalid`
/// when it is larger than the settings-file limit, not UTF-8 or not valid TOML.
pub fn read(path: &Path) -> Result<Table> {
    let at = || Error::new(codes::MANAGED_READ_FAILED).with_arg("path", path.display().to_string());
    let invalid =
        || Error::new(codes::MANAGED_INVALID).with_arg("path", path.display().to_string());
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Table::new()),
        Err(e) => return Err(at().with_source(e)),
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(invalid());
    }
    let bytes = fs::read(path).map_err(|e| at().with_source(e))?;
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.parse::<Table>().ok())
        .ok_or_else(invalid)
}

/// Reads the file at `path`, sets its values as the system scope of `store`, and locks the
/// listed keys. Unknown lock-list entries are logged with `tracing` and returned in
/// [`Managed::ignored`].
///
/// # Errors
/// As [`read`]; nothing is changed when reading fails.
pub fn apply(store: &mut SettingsStore, path: &Path) -> Result<Managed> {
    let table = read(path)?;
    let mut ignored = Vec::new();
    let mut requested = Vec::new();
    match table
        .get(MANAGED_TABLE)
        .and_then(Value::as_table)
        .and_then(|t| t.get(LOCKED))
    {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                match item.as_str() {
                    Some(key) => requested.push(key.to_owned()),
                    None => ignored.push("(not a text)".to_owned()),
                }
            }
        }
        Some(_) => ignored.push(format!("{MANAGED_TABLE}.{LOCKED}")),
    }
    let unknown = store.lock(requested.iter().map(String::as_str));
    let locked: Vec<String> = requested
        .into_iter()
        .filter(|key| !unknown.contains(key))
        .collect();
    ignored.extend(unknown);
    for key in &ignored {
        tracing::warn!(key = %key, "managed settings: lock entry ignored");
    }
    store.set_layer(Scope::System, table)?;
    Ok(Managed { locked, ignored })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Setting, SettingType, SettingValue, SettingsRegistry};
    use crate::testutil::dir;

    fn store_with_user(user: &str) -> SettingsStore {
        let mut store = SettingsStore::new(SettingsRegistry::builtin());
        store.set_layer(Scope::User, user.parse().unwrap()).unwrap();
        store
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    const USER: &str = "[log]\nlevel = \"debug\"\n[recent]\nmax_entries = 7\n";

    #[test]
    fn locked_key_takes_the_system_value_and_ignores_the_user_value() {
        let d = dir("managed-lock");
        let file = d.join("managed-settings.toml");
        write(
            &file,
            "[managed]\nlocked = [\"log.level\"]\n[log]\nlevel = \"warn\"\n",
        );
        let mut store = store_with_user(USER);
        let applied = apply(&mut store, &file).unwrap();
        assert_eq!(applied.locked, ["log.level"]);
        assert!(applied.ignored.is_empty());
        let e = store.get("log.level").unwrap();
        assert_eq!(
            (e.value, e.source),
            (SettingValue::Text("warn".into()), Scope::System)
        );
        assert_eq!(e.issues.len(), 1);
        assert_eq!(e.issues[0].kind, crate::settings::IssueKind::Locked);
        assert_eq!(e.issues[0].scope, Scope::User);
    }

    #[test]
    fn writes_below_the_system_scope_are_refused_and_name_the_key() {
        let d = dir("managed-write");
        let file = d.join("managed-settings.toml");
        write(&file, "[managed]\nlocked = [\"log.level\"]\n");
        let mut store = store_with_user(USER);
        apply(&mut store, &file).unwrap();
        let err = store
            .set(Scope::User, "log.level", SettingValue::Text("trace".into()))
            .unwrap_err();
        assert_eq!(err.code(), "app.settings.locked");
        assert_eq!(err.category(), arc_base::Category::PermissionDenied);
        assert_eq!(
            err.arguments().get("key"),
            Some(&arc_base::error::Value::Text("log.level".into()))
        );
        assert_eq!(
            store.reset(Scope::User, "log.level").unwrap_err().code(),
            "app.settings.locked"
        );
        // The refused write changed nothing.
        assert_eq!(
            store.layer(Scope::User).unwrap()["log"]["level"].as_str(),
            Some("debug")
        );
        // The system scope itself may still write the key.
        store
            .set(
                Scope::System,
                "log.level",
                SettingValue::Text("error".into()),
            )
            .unwrap();
        assert_eq!(
            store.get("log.level").unwrap().value,
            SettingValue::Text("error".into())
        );
    }

    #[test]
    fn unlocked_keys_are_unaffected() {
        let d = dir("managed-unlocked");
        let file = d.join("managed-settings.toml");
        write(&file, "[managed]\nlocked = [\"log.level\"]\n");
        let mut store = store_with_user(USER);
        apply(&mut store, &file).unwrap();
        let e = store.get("recent.max_entries").unwrap();
        assert_eq!((e.value, e.source), (SettingValue::Int(7), Scope::User));
        assert!(e.issues.is_empty());
        store
            .set(Scope::User, "recent.max_entries", SettingValue::Int(9))
            .unwrap();
        assert!(!store.is_locked("recent.max_entries"));
    }

    #[test]
    fn locked_key_without_a_system_value_takes_the_default() {
        let d = dir("managed-default");
        let file = d.join("managed-settings.toml");
        write(&file, "[managed]\nlocked = [\"log.level\"]\n");
        let mut store = store_with_user(USER);
        apply(&mut store, &file).unwrap();
        let e = store.get("log.level").unwrap();
        assert_eq!(
            (e.value, e.source),
            (SettingValue::Text("info".into()), Scope::Default)
        );
    }

    #[test]
    fn unknown_and_malformed_lock_entries_are_ignored() {
        let d = dir("managed-ignored");
        let file = d.join("managed-settings.toml");
        write(
            &file,
            "[managed]\nlocked = [\"nope.key\", 3, \"log.level\", \"managed.locked\"]\n",
        );
        let mut store = store_with_user(USER);
        let applied = apply(&mut store, &file).unwrap();
        assert_eq!(applied.locked, ["log.level"]);
        assert_eq!(
            applied.ignored,
            ["(not a text)", "nope.key", "managed.locked"]
        );
        write(&file, "[managed]\nlocked = \"log.level\"\n");
        let applied = apply(&mut store, &file).unwrap();
        assert!(applied.locked.is_empty());
        assert_eq!(applied.ignored, ["managed.locked"]);
    }

    #[test]
    fn missing_file_applies_nothing() {
        let d = dir("managed-missing");
        let mut store = store_with_user(USER);
        let applied = apply(&mut store, &d.join("none.toml")).unwrap();
        assert!(applied.locked.is_empty() && applied.ignored.is_empty());
        store
            .set(Scope::User, "log.level", SettingValue::Text("trace".into()))
            .unwrap();
    }

    #[test]
    fn unusable_file_is_an_error_and_changes_nothing() {
        let d = dir("managed-bad");
        let file = d.join("managed-settings.toml");
        write(&file, "[managed\nlocked = [");
        let mut store = store_with_user(USER);
        assert_eq!(
            apply(&mut store, &file).unwrap_err().code(),
            "app.managed.invalid"
        );
        assert!(store.layer(Scope::System).is_none());
        assert!(!store.is_locked("log.level"));
        assert!(file.exists(), "the admin file is never renamed");
    }

    #[test]
    fn the_managed_table_is_not_a_setting_namespace() {
        let mut registry = SettingsRegistry::new();
        let setting = Setting {
            key: "managed.locked",
            ty: SettingType::Bool,
            default: SettingValue::Bool(false),
            scope: Scope::User,
            message_id: "x",
        };
        assert_eq!(
            registry.register(setting).unwrap_err().code(),
            "app.settings.invalid_key"
        );
    }
}
