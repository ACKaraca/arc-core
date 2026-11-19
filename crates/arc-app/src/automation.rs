//! The automation switch (CORE-APP-020): the setting that starts and stops the local automation
//! socket, the per-client grants remembered in the user settings, and the socket's lifecycle.
//!
//! The setting `automation.enabled` (default off, user scope) decides whether the socket listens.
//! [`Automation::sync`] applies the effective value: on binds a [`LocalSocket`] in the given
//! directory and serves the automation protocol on a thread the module owns; off stops the thread
//! and removes the socket and token files.
//!
//! Grants are stored in the user settings file as `automation.grants`, an array of tables
//! `{ client = "<label>", permissions = ["read", ...] }`. The socket's ceiling is the union of the
//! granted permissions, read when the socket starts. `arc-command` gives every connection of a
//! socket the same ceiling and the same token and reports neither the client label nor
//! connections, so the grants cannot be enforced per client yet; see the specification.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use arc_base::{Error, Result};
use arc_command::protocol::{HANDSHAKE_TIMEOUT, LocalSocket, SocketConfig, SocketStop};
use arc_command::{Log, LogLevel, Permission, Registry, Server, ServerConfig};
use toml::{Table, Value};

use crate::codes;
use crate::product::ProductBuilder;
use crate::settings::{Scope, Setting, SettingType, SettingValue, SettingsStore};
use crate::settings_file;

/// The setting that enables the automation socket.
pub const ENABLED: &str = "automation.enabled";

/// The table of the user settings that holds the grants.
pub const TABLE: &str = "automation";

/// The key of the grants array inside [`TABLE`].
pub const GRANTS: &str = "grants";

/// The base name of the socket and token files.
pub const SOCKET_NAME: &str = "automation";

/// The most grants read from the settings; further entries are ignored.
pub const MAX_GRANTS: usize = 64;

/// The longest client label, in bytes.
pub const MAX_CLIENT_BYTES: usize = 64;

/// Registers [`ENABLED`]: a truth value, off by default, settable in the user scope.
///
/// # Errors
/// `app.settings.duplicate_key` when the product registered the key already.
pub fn register_settings(builder: &mut ProductBuilder) -> Result<()> {
    builder.setting(setting())
}

/// The definition of [`ENABLED`].
#[must_use]
pub fn setting() -> Setting {
    Setting {
        key: ENABLED,
        ty: SettingType::Bool,
        default: SettingValue::Bool(false),
        scope: Scope::User,
        message_id: "app-setting-automation-enabled",
    }
}

/// One client's granted permissions, keyed by the label the client chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The client label: 1 to [`MAX_CLIENT_BYTES`] bytes, no control characters.
    pub client: String,
    /// The permissions granted to the client; empty when only the entry is kept.
    pub permissions: BTreeSet<Permission>,
}

fn valid_label(label: &str) -> bool {
    !label.trim().is_empty()
        && label.len() <= MAX_CLIENT_BYTES
        && !label.chars().any(char::is_control)
}

fn permission_name(permission: Permission) -> Option<Value> {
    serde_json::to_value(permission)
        .ok()
        .and_then(|v| v.as_str().map(|s| Value::String(s.to_owned())))
}

fn permission_of(value: &Value) -> Option<Permission> {
    let text = value.as_str()?;
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The grants stored in the user scope. Malformed entries and repeated labels are skipped; the
/// first entry of a label wins.
#[must_use]
pub fn grants(store: &SettingsStore) -> Vec<Grant> {
    let Some(items) = store
        .layer(Scope::User)
        .and_then(|t| t.get(TABLE))
        .and_then(Value::as_table)
        .and_then(|t| t.get(GRANTS))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out: Vec<Grant> = Vec::new();
    for item in items.iter().take(MAX_GRANTS) {
        let Some(table) = item.as_table() else {
            continue;
        };
        let Some(client) = table.get("client").and_then(Value::as_str) else {
            continue;
        };
        if !valid_label(client) || out.iter().any(|g| g.client == client) {
            continue;
        }
        let permissions = table
            .get("permissions")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(permission_of).collect())
            .unwrap_or_default();
        out.push(Grant {
            client: client.to_owned(),
            permissions,
        });
    }
    out
}

/// The union of the permissions of `grants`: the ceiling of the socket.
#[must_use]
pub fn ceiling(grants: &[Grant]) -> BTreeSet<Permission> {
    grants
        .iter()
        .flat_map(|g| g.permissions.iter().copied())
        .collect()
}

/// Writes `grants` into the user table and saves it to `file` when one is given. The in-memory
/// value changes even when saving fails.
fn write(store: &mut SettingsStore, file: Option<&Path>, grants: &[Grant]) -> Result<()> {
    let mut table = store.layer(Scope::User).cloned().unwrap_or_default();
    let section = table
        .entry(TABLE.to_owned())
        .or_insert_with(|| Value::Table(Table::new()));
    let section = section
        .as_table_mut()
        .ok_or_else(|| Error::new(codes::SETTINGS_PATH_CONFLICT).with_arg("key", TABLE))?;
    let items = grants
        .iter()
        .map(|grant| {
            let mut entry = Table::new();
            entry.insert("client".into(), Value::String(grant.client.clone()));
            let names = grant.permissions.iter().filter_map(|p| permission_name(*p));
            entry.insert("permissions".into(), Value::Array(names.collect()));
            Value::Table(entry)
        })
        .collect();
    section.insert(GRANTS.to_owned(), Value::Array(items));
    store.set_layer(Scope::User, table.clone())?;
    match file {
        Some(file) => settings_file::save(file, &table),
        None => Ok(()),
    }
}

/// Grants `permissions` to the client `client`, replacing an earlier grant of that label. An
/// empty set revokes it.
///
/// # Errors
/// `app.automation.client_invalid` for a label that is empty, too long or has control
/// characters; `app.settings.path_conflict` when `automation` is not a table; and
/// `app.settings_file.write_failed` after the in-memory change.
pub fn grant(
    store: &mut SettingsStore,
    file: Option<&Path>,
    client: &str,
    permissions: BTreeSet<Permission>,
) -> Result<()> {
    if !valid_label(client) {
        return Err(Error::new(codes::AUTOMATION_CLIENT_INVALID));
    }
    let mut all: Vec<Grant> = grants(store)
        .into_iter()
        .filter(|g| g.client != client)
        .collect();
    if !permissions.is_empty() {
        all.push(Grant {
            client: client.to_owned(),
            permissions,
        });
    }
    write(store, file, &all)
}

/// Removes the grant of `client`. Returns whether one existed.
///
/// # Errors
/// As [`grant`].
pub fn revoke(store: &mut SettingsStore, file: Option<&Path>, client: &str) -> Result<bool> {
    let before = grants(store);
    let after: Vec<Grant> = before
        .iter()
        .filter(|g| g.client != client)
        .cloned()
        .collect();
    if after.len() == before.len() {
        return Ok(false);
    }
    write(store, file, &after)?;
    Ok(true)
}

/// Sends the protocol's log records to `tracing`. Only the event name is recorded: the detail of
/// some events is a client label, which is not written to the log.
struct TraceLog;

impl Log for TraceLog {
    fn log(&self, level: LogLevel, event: &str, _detail: &str) {
        match level {
            LogLevel::Debug => tracing::debug!(event = event, "automation"),
            LogLevel::Info => tracing::info!(event = event, "automation"),
            LogLevel::Warning => tracing::warn!(event = event, "automation"),
            LogLevel::Error => tracing::error!(event = event, "automation"),
        }
    }
}

/// A running socket: the handle that ends its serving thread and the socket itself.
struct Running {
    socket: Arc<LocalSocket>,
    stop: SocketStop,
    worker: JoinHandle<()>,
}

/// The automation socket of the application.
#[derive(Default)]
pub struct Automation {
    running: Option<Running>,
}

impl std::fmt::Debug for Automation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Automation")
            .field("running", &self.is_running())
            .finish()
    }
}

impl Automation {
    /// A switch that is off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the socket listens.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The socket path while it listens.
    #[must_use]
    pub fn socket_path(&self) -> Option<&Path> {
        self.running.as_ref().map(|r| r.socket.socket_path())
    }

    /// The token file while the socket listens; clients read the token from it.
    #[must_use]
    pub fn token_path(&self) -> Option<&Path> {
        self.running.as_ref().map(|r| r.socket.token_path())
    }

    /// Binds the socket in `directory` and serves the protocol of `registry` as product
    /// `name`/`version` with `ceiling` on a thread. Does nothing when the socket already listens.
    ///
    /// # Errors
    /// The errors of `arc_command`'s `LocalSocket::bind` (`cmd.socket.*`, for example
    /// `cmd.socket.directory` when the directory is not private) and `app.automation.failed`
    /// when the thread cannot start; a failed start leaves no files behind.
    pub fn start(
        &mut self,
        directory: &Path,
        registry: Registry,
        name: &str,
        version: &str,
        ceiling: BTreeSet<Permission>,
    ) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let config = SocketConfig {
            enabled: true,
            directory: PathBuf::from(directory),
            name: SOCKET_NAME.to_owned(),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        };
        let socket = Arc::new(LocalSocket::bind(&config)?);
        let stop = socket.stopper();
        let serving = Arc::clone(&socket);
        let (name, version) = (name.to_owned(), version.to_owned());
        let worker = thread::Builder::new()
            .name("arc-automation".into())
            .spawn(move || {
                let server_config = ServerConfig::new(name, version, ceiling);
                // The socket serves no open document yet: document commands answer with an
                // unknown-document error.
                let documents = BTreeMap::new();
                let server = Server::new(&registry, &documents, server_config, &TraceLog);
                if let Err(e) = serving.serve(&server) {
                    tracing::warn!(error = %e, "automation socket stopped with an error");
                }
            })
            .map_err(|e| Error::new(codes::AUTOMATION_FAILED).with_source(e))?;
        self.running = Some(Running {
            socket,
            stop,
            worker,
        });
        Ok(())
    }

    /// Ends the serving thread, closes its connections and removes the socket and token files.
    /// Does nothing when the socket does not listen.
    pub fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            let Running {
                socket,
                stop,
                worker,
            } = running;
            stop.stop();
            let _ = worker.join();
            // The serving thread held the second handle; dropping the last one removes the files.
            drop(socket);
        }
    }

    /// Applies the effective value of [`ENABLED`] in `store`: starts the socket with the ceiling
    /// of the current grants when it is on, stops it when it is off.
    ///
    /// # Errors
    /// `app.settings.unknown_key` when the key is not registered, and the errors of
    /// [`start`](Self::start).
    pub fn sync(
        &mut self,
        store: &SettingsStore,
        directory: &Path,
        registry: Registry,
        name: &str,
        version: &str,
    ) -> Result<()> {
        if store.get(ENABLED)?.value == SettingValue::Bool(true) {
            self.start(directory, registry, name, version, ceiling(&grants(store)))
        } else {
            self.stop();
            Ok(())
        }
    }
}

impl Drop for Automation {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed;
    use crate::settings::SettingsRegistry;
    use crate::testutil::dir;

    fn fresh() -> SettingsStore {
        let mut registry = SettingsRegistry::builtin();
        registry.register(setting()).unwrap();
        SettingsStore::new(registry)
    }

    fn set_on(store: &mut SettingsStore, on: bool) -> Result<()> {
        store.set(Scope::User, ENABLED, SettingValue::Bool(on))
    }

    #[test]
    fn the_switch_is_off_by_default() {
        let store = fresh();
        assert_eq!(store.get(ENABLED).unwrap().value, SettingValue::Bool(false));
        let d = dir("automation-default");
        let mut automation = Automation::new();
        automation
            .sync(&store, &d.join("run"), Registry::new(), "arc", "0")
            .unwrap();
        assert!(!automation.is_running());
        assert!(!d.join("run").exists(), "nothing is created while off");
    }

    #[test]
    #[cfg(unix)]
    fn turning_on_binds_and_serves_and_turning_off_removes_the_files() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let d = dir("automation-switch");
        let run = d.join("run");
        let mut store = fresh();
        let mut automation = Automation::new();
        set_on(&mut store, true).unwrap();
        automation
            .sync(&store, &run, Registry::new(), "arc-test", "0")
            .unwrap();
        assert!(automation.is_running());
        let socket = automation.socket_path().unwrap().to_owned();
        let token = std::fs::read_to_string(automation.token_path().unwrap()).unwrap();
        assert!(socket.exists());

        // The socket answers the transport handshake of the protocol.
        let mut conn = UnixStream::connect(&socket).unwrap();
        writeln!(
            conn,
            "{{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"arc.authenticate\",\"params\":{{\"token\":\"{token}\"}}}}"
        )
        .unwrap();
        let mut answer = String::new();
        BufReader::new(conn.try_clone().unwrap())
            .read_line(&mut answer)
            .unwrap();
        assert!(
            answer.contains("\"id\":7") && answer.contains("result"),
            "{answer}"
        );
        drop(conn);

        set_on(&mut store, false).unwrap();
        automation
            .sync(&store, &run, Registry::new(), "arc-test", "0")
            .unwrap();
        assert!(!automation.is_running());
        assert!(!socket.exists(), "the socket file is gone");
        assert!(
            !run.join("automation.token").exists(),
            "the token file is gone"
        );
    }

    #[test]
    fn the_switch_cannot_be_turned_on_when_the_system_locks_it_off() {
        let d = dir("automation-locked");
        let file = d.join("managed-settings.toml");
        std::fs::write(
            &file,
            "[managed]\nlocked = [\"automation.enabled\"]\n[automation]\nenabled = false\n",
        )
        .unwrap();
        let mut store = fresh();
        managed::apply(&mut store, &file).unwrap();
        let err = set_on(&mut store, true).unwrap_err();
        assert_eq!(err.code(), "app.settings.locked");
        let mut automation = Automation::new();
        automation
            .sync(&store, &d.join("run"), Registry::new(), "arc", "0")
            .unwrap();
        assert!(!automation.is_running());
    }

    #[test]
    fn grants_are_remembered_in_the_user_settings_and_revocable() {
        let d = dir("automation-grants");
        let file = d.join("settings.toml");
        let mut store = fresh();
        let read_edit: BTreeSet<Permission> = [Permission::Read, Permission::Edit].into();
        grant(&mut store, Some(&file), "my script", read_edit.clone()).unwrap();
        grant(
            &mut store,
            Some(&file),
            "other",
            [Permission::FileRead].into(),
        )
        .unwrap();
        assert_eq!(
            grants(&store),
            [
                Grant {
                    client: "my script".into(),
                    permissions: read_edit.clone(),
                },
                Grant {
                    client: "other".into(),
                    permissions: [Permission::FileRead].into(),
                },
            ]
        );
        assert_eq!(
            ceiling(&grants(&store)),
            [Permission::Read, Permission::Edit, Permission::FileRead].into()
        );

        // The grants survive a restart: the saved file is read into a new store.
        let mut restarted = fresh();
        restarted
            .set_layer(Scope::User, settings_file::load(&file).unwrap().table)
            .unwrap();
        assert_eq!(grants(&restarted).len(), 2);

        assert!(revoke(&mut restarted, Some(&file), "my script").unwrap());
        assert!(!revoke(&mut restarted, Some(&file), "my script").unwrap());
        let reloaded = settings_file::load(&file).unwrap().table;
        assert_eq!(
            reloaded["automation"]["grants"].as_array().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn invalid_labels_and_malformed_entries_are_refused_or_skipped() {
        let mut store = fresh();
        for label in ["", "   ", "tab\tname", &"x".repeat(MAX_CLIENT_BYTES + 1)] {
            assert_eq!(
                grant(&mut store, None, label, [Permission::Read].into())
                    .unwrap_err()
                    .code(),
                "app.automation.client_invalid"
            );
        }
        store
            .set_layer(
                Scope::User,
                "[automation]\nenabled = true\ngrants = [\"x\", { client = \"ok\", permissions = [\"read\", \"bogus\"] }, { client = \"ok\", permissions = [\"edit\"] }]\n"
                    .parse()
                    .unwrap(),
            )
            .unwrap();
        let listed = grants(&store);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].permissions, [Permission::Read].into());
        assert_eq!(
            store.get(ENABLED).unwrap().value,
            SettingValue::Bool(true),
            "grants and the switch share the table without interfering"
        );
    }
}
