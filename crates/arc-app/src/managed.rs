//! Managed settings (CORE-APP-013): the administrator's system-scope settings file.
//!
//! The file is a TOML table like a user settings file. Its values are the system scope. A
//! top-level table `managed` holds the lock list, `managed.locked`, an array of setting keys:
//! a locked key takes its value from the system scope only, and a write to it outside the
//! system scope is refused with `app.settings.locked`. A locked key with no system value takes
//! the default. Entries that are not registered keys are ignored and logged.
//!
//! The file is read, never written or renamed: it lives in a directory the user does not own. A
//! missing file applies nothing. A file that exists but cannot be used stops the application from
//! starting ([`is_fatal`]): nothing from it is applied and the application does not fall back to
//! running without the administrator's locks.

use std::fs;
use std::path::Path;

use arc_base::{Error, Result};
use toml::{Table, Value};

use crate::codes;
use crate::settings::{MANAGED_TABLE, SettingsStore};
use crate::settings_file::MAX_FILE_BYTES;

/// The key of the lock list inside [`MANAGED_TABLE`].
pub const LOCKED: &str = "locked";

/// The owner the administrator file and its folder must have on Unix: the superuser.
#[cfg(unix)]
const ADMINISTRATOR_UID: u32 = 0;

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
/// listed keys, replacing the lock set of an earlier apply. Unknown lock-list entries are logged
/// with `tracing` and returned in [`Managed::ignored`].
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
    let keys: Vec<&str> = requested.iter().map(String::as_str).collect();
    let unknown = store.set_system(table, &keys)?;
    let locked: Vec<String> = requested
        .into_iter()
        .filter(|key| !unknown.contains(key))
        .collect();
    ignored.extend(unknown);
    for key in &ignored {
        tracing::warn!(key = %key, "managed settings: lock entry ignored");
    }
    Ok(Managed { locked, ignored })
}

/// Reads and applies the administrator file at `path` as [`apply`] does, after checking where it
/// lives. On Unix the file and its folder must be owned by the superuser and not writable by
/// group or other users; otherwise the file is refused with `app.managed.untrusted` and nothing is
/// read.
///
/// # Errors
/// As [`apply`], and `app.managed.untrusted`. Every one of them is fatal ([`is_fatal`]).
pub fn apply_system(store: &mut SettingsStore, path: &Path) -> Result<Managed> {
    #[cfg(unix)]
    require_owner(path, ADMINISTRATOR_UID)?;
    apply(store, path)
}

/// Whether `error` from [`apply_system`] must stop the application from starting. Every error of
/// this module is fatal: a file that exists but cannot be used is never skipped (fail closed).
#[must_use]
pub fn is_fatal(error: &Error) -> bool {
    [
        codes::MANAGED_READ_FAILED.code,
        codes::MANAGED_INVALID.code,
        codes::MANAGED_UNTRUSTED.code,
    ]
    .contains(&error.code())
}

/// Refuses a file that `owner` does not own, that is not a regular file (a symbolic link is
/// refused), or that group or other users can write, and a folder with the same faults. A missing
/// file passes: nothing is managed.
#[cfg(unix)]
fn require_owner(path: &Path, owner: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let read_failed = |e: std::io::Error| {
        Error::new(codes::MANAGED_READ_FAILED)
            .with_arg("path", path.display().to_string())
            .with_source(e)
    };
    let file = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_failed(e)),
    };
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let folder = fs::metadata(folder).map_err(read_failed)?;
    // Group and other write bits (0o022) must be clear.
    let trusted = |meta: &fs::Metadata| meta.uid() == owner && meta.mode() & 0o022 == 0;
    if file.is_file() && trusted(&file) && trusted(&folder) {
        return Ok(());
    }
    Err(Error::new(codes::MANAGED_UNTRUSTED).with_arg("path", path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Scope, Setting, SettingType, SettingValue, SettingsRegistry};
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
        let err = apply(&mut store, &file).unwrap_err();
        assert_eq!(err.code(), "app.managed.invalid");
        assert!(is_fatal(&err), "the caller must refuse to start");
        assert!(store.layer(Scope::System).is_none());
        assert!(!store.is_locked("log.level"));
        assert!(file.exists(), "the admin file is never renamed");
    }

    #[test]
    fn reapplying_an_edited_file_unlocks_keys_removed_from_it() {
        let d = dir("managed-relock");
        let file = d.join("managed-settings.toml");
        write(&file, "[managed]\nlocked = [\"log.level\"]\n");
        let mut store = store_with_user(USER);
        apply(&mut store, &file).unwrap();
        assert!(store.is_locked("log.level"));
        write(&file, "[managed]\nlocked = [\"recent.max_entries\"]\n");
        apply(&mut store, &file).unwrap();
        assert!(!store.is_locked("log.level"));
        assert!(store.is_locked("recent.max_entries"));
    }

    #[test]
    fn an_unusable_reapplied_file_keeps_the_previous_locks_and_values() {
        let d = dir("managed-keep");
        let file = d.join("managed-settings.toml");
        write(
            &file,
            "[managed]\nlocked = [\"log.level\"]\n[log]\nlevel = \"warn\"\n",
        );
        let mut store = store_with_user(USER);
        apply(&mut store, &file).unwrap();
        write(&file, "[managed\nlocked = [");
        assert!(apply(&mut store, &file).is_err());
        assert!(store.is_locked("log.level"));
        assert_eq!(
            store.get("log.level").unwrap().value,
            SettingValue::Text("warn".into())
        );
    }

    #[test]
    fn a_missing_file_is_not_fatal_and_applies_nothing() {
        let d = dir("managed-fatal-none");
        let mut store = store_with_user(USER);
        let applied = apply_system(&mut store, &d.join("absent.toml")).unwrap();
        assert!(applied.locked.is_empty());
    }

    #[cfg(unix)]
    mod trust {
        use super::*;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::path::PathBuf;

        /// A settings file in a folder, both owned by the test user; returns the file and the uid.
        fn owned(name: &str) -> (PathBuf, u32) {
            let d = dir(name);
            let file = d.join("managed-settings.toml");
            write(&file, "[managed]\nlocked = []\n");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
            let uid = fs::metadata(&file).unwrap().uid();
            (file, uid)
        }

        #[test]
        fn a_file_of_the_given_owner_in_a_private_folder_is_accepted() {
            let (file, uid) = owned("managed-trust-ok");
            require_owner(&file, uid).unwrap();
        }

        #[test]
        fn a_group_writable_file_is_refused() {
            let (file, uid) = owned("managed-trust-group");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o664)).unwrap();
            assert_eq!(
                require_owner(&file, uid).unwrap_err().code(),
                "app.managed.untrusted"
            );
        }

        #[test]
        fn a_world_writable_folder_is_refused() {
            let (file, uid) = owned("managed-trust-folder");
            let folder = file.parent().unwrap();
            fs::set_permissions(folder, fs::Permissions::from_mode(0o757)).unwrap();
            assert_eq!(
                require_owner(&file, uid).unwrap_err().code(),
                "app.managed.untrusted"
            );
        }

        #[test]
        fn a_file_of_another_owner_is_refused() {
            let (file, uid) = owned("managed-trust-owner");
            assert_eq!(
                require_owner(&file, uid.wrapping_add(1))
                    .unwrap_err()
                    .code(),
                "app.managed.untrusted"
            );
        }

        #[test]
        fn a_symbolic_link_is_refused_and_a_missing_file_passes() {
            let (file, uid) = owned("managed-trust-link");
            let link = file.with_file_name("link.toml");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert_eq!(
                require_owner(&link, uid).unwrap_err().code(),
                "app.managed.untrusted"
            );
            require_owner(&file.with_file_name("none.toml"), uid).unwrap();
        }
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
