//! Local socket sessions (CORE-CMD-020).
//!
//! A [`LocalSocket`] listens on a Unix domain socket inside a directory that only the user can
//! enter and serves the automation protocol, unchanged, to every client that authenticates. There
//! is no other listener. Two authentication modes exist:
//!
//! - [`LocalSocket::bind`]: one session token, written to `<name>.token`, admits every client and
//!   every connection runs with the server's ceiling.
//! - [`LocalSocket::bind_clients`]: a table of clients, each with its own token file
//!   (`<name>@<client>.token`) and its own permission ceiling. Clients are added and removed with
//!   [`LocalSocket::add_client`] and [`LocalSocket::remove_client`] while the socket serves.
//!   The `@` separator is outside the name alphabet, so no socket's file can be another's; the
//!   client files an earlier instance of the same name left behind are removed when it binds.
//!
//! Tokens are random, written with owner-only permissions, compared in constant time and never
//! logged or sent. The first line of a connection is the transport handshake `arc.authenticate`;
//! a connection that fails it is answered with `cmd.socket.unauthorized` alone and closed. Named
//! pipes for Windows are not provided yet: [`LocalSocket::bind`] answers `cmd.socket.unsupported`
//! there.

use std::path::PathBuf;
use std::time::Duration;

use arc_base::Error;

use crate::error::codes;

/// The most simultaneous connections; further ones are closed at once.
pub const MAX_CONNECTIONS: usize = 16;

/// The most connections that may wait in their handshake at once. The rest of the connection
/// slots stay free for clients that already authenticated, so silent connections cannot hold
/// every slot until their handshake deadline.
pub const MAX_HANDSHAKING: usize = MAX_CONNECTIONS / 2;

/// The most clients a client table holds; `add_client` beyond it is refused.
pub const MAX_CLIENTS: usize = 256;

/// The longest handshake line, in bytes.
pub const MAX_HANDSHAKE_BYTES: usize = 1024;

/// The longest client name a handshake may carry, in bytes.
pub const MAX_CLIENT_NAME_BYTES: usize = 256;

/// The default handshake deadline: ten seconds from the accept to the end of the handshake line.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Told when a connection of a [`LocalSocket`] passes its handshake and when it ends. Both calls
/// run on the connection's thread, outside every lock of the socket.
pub trait ConnectionObserver: Sync {
    /// A connection passed its handshake. `connection` identifies it for the rest of the socket's
    /// life and is never reused; `client_name` is the name the handshake carried (at most
    /// [`MAX_CLIENT_NAME_BYTES`] bytes, control characters replaced), empty when none was given.
    fn opened(&self, connection: u64, client_name: String);

    /// The connection `opened` reported has ended, for whatever reason. Called exactly once per
    /// such connection, after `opened`.
    fn closed(&self, connection: u64);
}

/// An observer that ignores every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoObserver;

impl ConnectionObserver for NoObserver {
    fn opened(&self, _: u64, _: String) {}

    fn closed(&self, _: u64) {}
}

/// What to listen on.
#[derive(Debug, Clone)]
pub struct SocketConfig {
    /// Whether the user enabled automation. Nothing is created or listened on when false.
    pub enabled: bool,
    /// The private directory holding the socket and the token file. It is created with mode
    /// 0700 when missing (its parent must exist); an existing one must be a directory that
    /// group and others cannot access.
    pub directory: PathBuf,
    /// The base name of the files: 1 to 64 characters of lowercase ASCII letters, digits, `_`,
    /// `-` and `.`, not starting with a dot. The socket is `<name>.sock`, the token file
    /// `<name>.token`.
    pub name: String,
    /// The time a connection has to send its handshake line, counted from the accept.
    pub handshake_timeout: Duration,
}

fn name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

/// Compares two byte strings in time that depends only on their length.
#[cfg(unix)]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Refuses a token file whose owner differs from the owner of its directory.
#[cfg(unix)]
fn same_owner(directory_uid: u32, file_uid: u32) -> Result<(), Error> {
    if directory_uid == file_uid {
        Ok(())
    } else {
        Err(directory_error("owner"))
    }
}

fn directory_error(reason: &'static str) -> Error {
    Error::new(codes::SOCKET_DIRECTORY).with_detail("reason", reason)
}

#[cfg(unix)]
fn io_error(reason: &'static str) -> Error {
    Error::new(codes::SOCKET_IO).with_detail("reason", reason)
}

fn check_enabled(config: &SocketConfig) -> Result<(), Error> {
    if !config.enabled {
        return Err(Error::new(codes::SOCKET_DISABLED));
    }
    if !name_ok(&config.name) {
        return Err(directory_error("name"));
    }
    Ok(())
}

#[cfg(unix)]
pub use unix::{LocalSocket, SocketStop};

#[cfg(unix)]
mod unix {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt;
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use arc_base::Error;
    use serde_json::Value;

    use super::{
        ConnectionObserver, MAX_CLIENT_NAME_BYTES, MAX_CLIENTS, MAX_CONNECTIONS,
        MAX_HANDSHAKE_BYTES, MAX_HANDSHAKING, NoObserver, SocketConfig, check_enabled,
        constant_time_eq, directory_error, io_error, name_ok, same_owner,
    };
    use crate::error::codes;
    use crate::protocol::{LogLevel, Server, response};
    use crate::spec::Permission;

    /// The length of every token, in bytes (64 hexadecimal digits).
    const TOKEN_BYTES: usize = 64;

    /// A client of the table: its token, its ceiling, a serial that tells a removed and re-added
    /// client of one name apart, and the file its token was written to.
    struct Client {
        token: String,
        ceiling: BTreeSet<Permission>,
        serial: u64,
        token_path: PathBuf,
    }

    /// A connection the socket holds: the client it authenticated as, if any, a handle that ends
    /// it, whether it still waits in its handshake, and the flag that revokes its session.
    struct Open {
        client: Option<(String, u64)>,
        stream: UnixStream,
        handshaking: bool,
        /// Set when the client is removed or the socket stops. The session then reads no further
        /// request and starts no queued run; a run already running is cancelled by the end of
        /// its reader.
        revoked: Arc<AtomicBool>,
    }

    /// The mutable state of a socket, behind one lock that is never held across I/O that waits
    /// or across an observer call.
    #[derive(Default)]
    struct Table {
        clients: BTreeMap<String, Client>,
        open: BTreeMap<u64, Open>,
        /// The connections in `open` that wait in their handshake.
        handshaking: usize,
        next_connection: u64,
        next_serial: u64,
    }

    /// A listening local socket with its token file. Dropping it removes the files `bind` and
    /// `add_client` created.
    pub struct LocalSocket {
        listener: UnixListener,
        directory: PathBuf,
        name: String,
        socket_path: PathBuf,
        token_path: PathBuf,
        /// The session token of single-token mode; `None` in client-table mode.
        token: Option<String>,
        handshake_timeout: Duration,
        stop: Arc<AtomicBool>,
        table: Mutex<Table>,
    }

    impl fmt::Debug for LocalSocket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("LocalSocket")
                .field("socket_path", &self.socket_path)
                .field("token_path", &self.token_path)
                .finish_non_exhaustive()
        }
    }

    /// Ends [`LocalSocket::serve`] from another thread.
    #[derive(Debug, Clone)]
    pub struct SocketStop {
        flag: Arc<AtomicBool>,
        path: PathBuf,
    }

    impl SocketStop {
        /// Asks the serving loop to end: open connections are closed and `serve` returns.
        pub fn stop(&self) {
            self.flag.store(true, Ordering::Release);
            // Wakes the blocked accept.
            let _ = UnixStream::connect(&self.path);
        }
    }

    /// 32 random bytes from the operating system, as lowercase hexadecimal.
    fn new_token() -> io::Result<String> {
        let mut bytes = [0u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Makes sure `directory` exists and only the user can enter it.
    fn private_directory(directory: &Path) -> Result<(), Error> {
        match fs::symlink_metadata(directory) {
            Ok(meta) => {
                if !meta.is_dir() {
                    return Err(directory_error("not_directory"));
                }
                if meta.permissions().mode() & 0o077 != 0 {
                    return Err(directory_error("not_private"));
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .map_err(|_| directory_error("create")),
            Err(_) => Err(directory_error("create")),
        }
    }

    /// Writes a new token file at `path`, which must not exist, and checks that its owner is
    /// the owner of `directory`. A file that fails either step is removed again.
    fn write_token(directory: &Path, path: &Path) -> Result<String, Error> {
        let token = new_token().map_err(|_| io_error("token"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| io_error("token"))?;
        let checked = file
            .write_all(token.as_bytes())
            .map_err(|_| io_error("token"))
            .and_then(|()| owner_matches(&file, directory));
        if checked.is_err() {
            let _ = fs::remove_file(path);
        }
        checked.map(|()| token)
    }

    fn owner_matches(file: &File, directory: &Path) -> Result<(), Error> {
        let file_uid = file.metadata().map_err(|_| io_error("token"))?.uid();
        let directory_uid = fs::metadata(directory)
            .map_err(|_| directory_error("create"))?
            .uid();
        same_owner(directory_uid, file_uid)
    }

    /// Removes the client token files that an earlier instance of socket `name` left behind: the
    /// regular files `<name>@<client>.token` owned by the owner of `directory`. Links and other
    /// entries are skipped, and so is anything that cannot be inspected or removed.
    fn remove_client_files(directory: &Path, name: &str) {
        let Ok(owner) = fs::metadata(directory) else {
            return;
        };
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let prefix = format!("{name}@");
        for entry in entries.flatten() {
            let Some(file) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let is_client_file = file
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".token"))
                .is_some_and(|client| !client.is_empty());
            if !is_client_file {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_file() && same_owner(owner.uid(), meta.uid()).is_ok() {
                let _ = fs::remove_file(&path);
            }
        }
    }

    /// A client name for the observer: control characters replaced. The handshake bounds the
    /// length, so the result is bounded too.
    fn shown(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_control() {
                    char::REPLACEMENT_CHARACTER
                } else {
                    c
                }
            })
            .collect()
    }

    /// Reads from a connection. While a deadline is set, each read waits no longer than the time
    /// left before it, so the whole handshake line shares one bound.
    struct Until<'a> {
        stream: &'a UnixStream,
        deadline: Option<Instant>,
    }

    impl Read for Until<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(deadline) = self.deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                self.stream.set_read_timeout(Some(left))?;
            }
            let mut stream = self.stream;
            stream.read(buf)
        }
    }

    /// What a handshake that passed grants.
    struct Admitted {
        /// The id of the handshake, echoed in its answer.
        id: Value,
        /// The client name the handshake carried, empty when none was given.
        name: String,
        /// The table entry it authenticated against: name and serial.
        entry: Option<(String, u64)>,
        /// The client's ceiling in client-table mode; `None` in single-token mode, where the
        /// server's own ceiling applies.
        ceiling: Option<BTreeSet<Permission>>,
    }

    impl LocalSocket {
        /// Binds the socket, then creates the token file, and returns the socket ready to
        /// accept, in single-token mode. A failed call removes only the files it created.
        ///
        /// # Errors
        ///
        /// `cmd.socket.disabled` when automation is not enabled; `cmd.socket.directory` (detail
        /// `reason`: `name`, `not_directory`, `not_private`, `create`, `owner`); `cmd.socket.io`
        /// (`in_use` when a live socket of that name exists, `exists`, `bind`, `token`).
        pub fn bind(config: &SocketConfig) -> Result<Self, Error> {
            Self::bind_as(config, false)
        }

        /// Binds the socket in client-table mode: no session token is created, every client is
        /// added with [`add_client`](Self::add_client), and a connection authenticates as the
        /// client whose name and token it presents. The client files of an earlier instance of
        /// the name are removed first, once no live socket of that name answers. Errors as
        /// [`bind`](Self::bind).
        ///
        /// # Errors
        ///
        /// As [`bind`](Self::bind).
        pub fn bind_clients(config: &SocketConfig) -> Result<Self, Error> {
            Self::bind_as(config, true)
        }

        fn bind_as(config: &SocketConfig, clients: bool) -> Result<Self, Error> {
            check_enabled(config)?;
            private_directory(&config.directory)?;
            let socket_path = config.directory.join(format!("{}.sock", config.name));
            let token_path = config.directory.join(format!("{}.token", config.name));
            match fs::symlink_metadata(&socket_path) {
                Ok(meta) if meta.file_type().is_socket() => {
                    if UnixStream::connect(&socket_path).is_ok() {
                        return Err(io_error("in_use"));
                    }
                    fs::remove_file(&socket_path).map_err(|_| io_error("exists"))?;
                    // The instance that left this socket behind ended without removing its token.
                    let _ = fs::remove_file(&token_path);
                }
                Ok(_) => return Err(io_error("exists")),
                Err(_) => {}
            }
            if clients {
                remove_client_files(&config.directory, &config.name);
            }
            let listener = UnixListener::bind(&socket_path).map_err(|_| io_error("bind"))?;
            // From here the socket is this call's: a failure below removes it and nothing else.
            let prepared = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
                .map_err(|_| io_error("bind"))
                .and_then(|()| {
                    if clients {
                        Ok(None)
                    } else {
                        write_token(&config.directory, &token_path).map(Some)
                    }
                });
            match prepared {
                Ok(token) => Ok(Self {
                    listener,
                    directory: config.directory.clone(),
                    name: config.name.clone(),
                    socket_path,
                    token_path,
                    token,
                    handshake_timeout: config.handshake_timeout,
                    stop: Arc::default(),
                    table: Mutex::default(),
                }),
                Err(error) => {
                    let _ = fs::remove_file(&socket_path);
                    Err(error)
                }
            }
        }

        /// The path of the socket.
        #[must_use]
        pub fn socket_path(&self) -> &Path {
            &self.socket_path
        }

        /// The path of the single-token file (`<name>.token`). It exists only in single-token
        /// mode; in client-table mode the client files are returned by
        /// [`add_client`](Self::add_client).
        #[must_use]
        pub fn token_path(&self) -> &Path {
            &self.token_path
        }

        /// The path of the token file of client `client`, as [`add_client`](Self::add_client)
        /// writes it. The file exists only while the client does.
        #[must_use]
        pub fn client_token_path(&self, client: &str) -> PathBuf {
            self.directory.join(format!("{}@{client}.token", self.name))
        }

        /// A handle that ends [`serve`](Self::serve).
        #[must_use]
        pub fn stopper(&self) -> SocketStop {
            SocketStop {
                flag: Arc::clone(&self.stop),
                path: self.socket_path.clone(),
            }
        }

        /// Adds a client of the table: writes its token to `<name>@<client>.token` (owner-only,
        /// created exclusively) and grants it `ceiling`. The connections of the client run with
        /// the intersection of `ceiling` and the server's ceiling.
        ///
        /// The client files of an earlier instance of this socket name are removed when the socket
        /// is bound, so a file of that name found here was written by this socket.
        ///
        /// # Errors
        ///
        /// `cmd.socket.io` with detail `reason`: `single_token` (the socket is in single-token
        /// mode), `client_name` (not 1 to 64 characters of the socket name's alphabet), `exists`
        /// (the client is in the table already), `clients` (the table holds [`MAX_CLIENTS`]
        /// clients) or `token` (the file could not be written).
        pub fn add_client(
            &self,
            client: &str,
            ceiling: BTreeSet<Permission>,
        ) -> Result<PathBuf, Error> {
            if self.token.is_some() {
                return Err(io_error("single_token"));
            }
            if !name_ok(client) {
                return Err(io_error("client_name"));
            }
            let token_path = self.client_token_path(client);
            let mut table = self.lock();
            if table.clients.contains_key(client) {
                return Err(io_error("exists"));
            }
            if table.clients.len() >= MAX_CLIENTS {
                return Err(io_error("clients"));
            }
            let token = write_token(&self.directory, &token_path)?;
            let serial = table.next_serial;
            table.next_serial = serial.wrapping_add(1);
            table.clients.insert(
                client.to_owned(),
                Client {
                    token,
                    ceiling,
                    serial,
                    token_path: token_path.clone(),
                },
            );
            Ok(token_path)
        }

        /// Removes a client of the table: its connections are revoked and closed, its next
        /// handshake is refused, and its token file is removed. Returns false when no such client
        /// exists.
        pub fn remove_client(&self, client: &str) -> bool {
            let mut table = self.lock();
            let Some(entry) = table.clients.remove(client) else {
                return false;
            };
            for open in table.open.values() {
                let owned = open
                    .client
                    .as_ref()
                    .is_some_and(|(name, serial)| name == client && *serial == entry.serial);
                if owned {
                    open.revoked.store(true, Ordering::Release);
                    let _ = open.stream.shutdown(std::net::Shutdown::Both);
                }
            }
            // Deleted under the lock, so an add of the same name cannot write its file first.
            let _ = fs::remove_file(&entry.token_path);
            true
        }

        /// Accepts connections until stopped, serving each one that authenticates as a protocol
        /// session of `server` on its own thread. Returns when stopped, after the open
        /// connections were closed and their sessions ended.
        ///
        /// # Errors
        ///
        /// An error of the listener other than an aborted or interrupted accept.
        pub fn serve(&self, server: &Server<'_>) -> io::Result<()> {
            self.serve_observed(server, &NoObserver)
        }

        /// [`serve`](Self::serve), telling `observer` when a connection passes its handshake and
        /// when it ends.
        ///
        /// # Errors
        ///
        /// As [`serve`](Self::serve).
        pub fn serve_observed(
            &self,
            server: &Server<'_>,
            observer: &dyn ConnectionObserver,
        ) -> io::Result<()> {
            let mut result = Ok(());
            thread::scope(|scope| {
                for incoming in self.listener.incoming() {
                    if self.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let stream = match incoming {
                        Ok(stream) => stream,
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted
                            ) =>
                        {
                            continue;
                        }
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    };
                    // Over a limit (or unclonable) the connection is closed unanswered.
                    let Some((connection, revoked)) = self.register(&stream) else {
                        continue;
                    };
                    let spawned = thread::Builder::new()
                        .name("arc-socket-session".into())
                        .spawn_scoped(scope, move || {
                            self.connection(server, observer, connection, revoked, stream);
                        });
                    if spawned.is_err() {
                        self.forget(connection);
                    }
                }
                for open in self.lock().open.values() {
                    open.revoked.store(true, Ordering::Release);
                    let _ = open.stream.shutdown(std::net::Shutdown::Both);
                }
            });
            result
        }

        fn lock(&self) -> MutexGuard<'_, Table> {
            self.table.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Records a new connection, unless the connection limit is reached or the handshake
        /// share is full. The identifier is new for the life of the socket.
        fn register(&self, stream: &UnixStream) -> Option<(u64, Arc<AtomicBool>)> {
            let mut table = self.lock();
            if table.open.len() >= MAX_CONNECTIONS || table.handshaking >= MAX_HANDSHAKING {
                return None;
            }
            let connection = table.next_connection;
            // An exhausted counter refuses connections rather than reusing an identifier.
            table.next_connection = connection.checked_add(1)?;
            let handle = stream.try_clone().ok()?;
            let revoked = Arc::new(AtomicBool::new(false));
            table.handshaking += 1;
            table.open.insert(
                connection,
                Open {
                    client: None,
                    stream: handle,
                    handshaking: true,
                    revoked: Arc::clone(&revoked),
                },
            );
            Some((connection, revoked))
        }

        /// Removes a connection from the table, releasing its handshake slot when it still had one.
        fn forget(&self, connection: u64) {
            let mut table = self.lock();
            if table
                .open
                .remove(&connection)
                .is_some_and(|open| open.handshaking)
            {
                table.handshaking = table.handshaking.saturating_sub(1);
            }
        }

        /// The grant of a handshake that is well formed and authenticates: in single-token mode
        /// the one token, in client-table mode the named client's token.
        fn handshake(&self, line: &[u8]) -> Option<Admitted> {
            let message: Value = serde_json::from_slice(line).ok()?;
            let params = message.get("params")?;
            let token = params.get("token")?.as_str()?;
            let name = match params.get("clientName") {
                None => None,
                Some(name) => Some(
                    name.as_str()
                        .filter(|n| (1..=MAX_CLIENT_NAME_BYTES).contains(&n.len()))?,
                ),
            };
            let authentic = message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && message.get("method").and_then(Value::as_str) == Some("arc.authenticate");
            let id = message.get("id")?;
            if !authentic || !(id.is_string() || id.is_i64() || id.is_u64()) {
                return None;
            }
            let id = id.clone();
            let table = self.lock();
            let admitted = match &self.token {
                Some(single) => {
                    if !constant_time_eq(token.as_bytes(), single.as_bytes()) {
                        return None;
                    }
                    Admitted {
                        id,
                        name: name.unwrap_or_default().to_owned(),
                        entry: None,
                        ceiling: None,
                    }
                }
                None => {
                    let name = name?;
                    let Some(client) = table.clients.get(name) else {
                        // The same comparison as for a known name, so the answer takes the same
                        // time whether or not the name exists.
                        let dummy = [b'0'; TOKEN_BYTES];
                        let _ = constant_time_eq(token.as_bytes(), &dummy);
                        return None;
                    };
                    if !constant_time_eq(token.as_bytes(), client.token.as_bytes()) {
                        return None;
                    }
                    Admitted {
                        id,
                        name: name.to_owned(),
                        entry: Some((name.to_owned(), client.serial)),
                        ceiling: Some(client.ceiling.clone()),
                    }
                }
            };
            Some(admitted)
        }

        /// Attaches an admitted handshake to its connection, which leaves the handshake share.
        /// False when the client was removed while the handshake ran, which refuses the connection.
        fn admit(&self, connection: u64, admitted: &Admitted) -> bool {
            let mut guard = self.lock();
            let table = &mut *guard;
            if let Some((name, serial)) = &admitted.entry {
                let current = table
                    .clients
                    .get(name)
                    .is_some_and(|client| client.serial == *serial);
                if !current {
                    return false;
                }
            }
            let Some(open) = table.open.get_mut(&connection) else {
                return false;
            };
            if let Some((name, serial)) = &admitted.entry {
                open.client = Some((name.clone(), *serial));
            }
            if std::mem::replace(&mut open.handshaking, false) {
                table.handshaking = table.handshaking.saturating_sub(1);
            }
            true
        }

        fn connection(
            &self,
            server: &Server<'_>,
            observer: &dyn ConnectionObserver,
            connection: u64,
            revoked: Arc<AtomicBool>,
            stream: UnixStream,
        ) {
            let deadline = Instant::now() + self.handshake_timeout;
            let mut reader = BufReader::new(Until {
                stream: &stream,
                deadline: Some(deadline),
            });
            // At most one byte past the limit is read: a line with no newline within the limit
            // ends the connection, and nothing beyond the handshake is consumed.
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(MAX_HANDSHAKE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line);
            let admitted = match read {
                Ok(_) if line.last() == Some(&b'\n') => self.handshake(&line),
                _ => None,
            }
            .filter(|admitted| self.admit(connection, admitted));
            let mut out = &stream;
            let Some(admitted) = admitted else {
                // Nothing but the documented error, then the connection ends.
                let refusal = response(
                    &Value::Null,
                    Err(Error::new(codes::SOCKET_UNAUTHORIZED).into()),
                );
                let _ = writeln!(out, "{refusal}");
                let _ = stream.shutdown(std::net::Shutdown::Both);
                server.log.log(LogLevel::Warning, "socket.unauthorized", "");
                self.forget(connection);
                return;
            };
            // A client's grant never exceeds the server's ceiling.
            let ceiling: BTreeSet<Permission> = match &admitted.ceiling {
                Some(client) => client
                    .intersection(&server.config.ceiling)
                    .copied()
                    .collect(),
                None => server.config.ceiling.clone(),
            };
            observer.opened(connection, shown(&admitted.name));
            let accepted = response(&admitted.id, Ok(serde_json::json!({})));
            // The session that follows is not bound by the handshake deadline.
            reader.get_mut().deadline = None;
            if writeln!(out, "{accepted}").is_ok() && stream.set_read_timeout(None).is_ok() {
                let _ = server.serve_with_ceiling(ceiling, reader, out, &revoked);
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
            observer.closed(connection);
            self.forget(connection);
        }
    }

    impl Drop for LocalSocket {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.socket_path);
            let _ = fs::remove_file(&self.token_path);
            let table = self.table.get_mut().unwrap_or_else(PoisonError::into_inner);
            for client in table.clients.values() {
                let _ = fs::remove_file(&client.token_path);
            }
        }
    }
}

#[cfg(not(unix))]
pub use stub::{LocalSocket, SocketStop};

#[cfg(not(unix))]
mod stub {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::{Path, PathBuf};

    use arc_base::Error;

    use super::{ConnectionObserver, NoObserver, SocketConfig, check_enabled};
    use crate::error::codes;
    use crate::protocol::Server;
    use crate::spec::Permission;

    /// Named pipes are not provided yet; no value of this type can be made.
    #[derive(Debug)]
    pub struct LocalSocket(());

    /// Ends [`LocalSocket::serve`] from another thread.
    #[derive(Debug, Clone)]
    pub struct SocketStop(());

    impl SocketStop {
        /// Does nothing: there is no socket.
        pub fn stop(&self) {}
    }

    impl LocalSocket {
        /// # Errors
        ///
        /// `cmd.socket.disabled` when automation is not enabled, otherwise
        /// `cmd.socket.unsupported`: named pipes are not provided on this platform yet.
        pub fn bind(config: &SocketConfig) -> Result<Self, Error> {
            check_enabled(config)?;
            Err(Error::new(codes::SOCKET_UNSUPPORTED))
        }

        /// # Errors
        ///
        /// As [`bind`](Self::bind).
        pub fn bind_clients(config: &SocketConfig) -> Result<Self, Error> {
            check_enabled(config)?;
            Err(Error::new(codes::SOCKET_UNSUPPORTED))
        }

        /// # Errors
        ///
        /// Always `cmd.socket.unsupported`: there is no socket.
        pub fn add_client(&self, _: &str, _: BTreeSet<Permission>) -> Result<PathBuf, Error> {
            Err(Error::new(codes::SOCKET_UNSUPPORTED))
        }

        /// Does nothing and reports false: there is no socket.
        pub fn remove_client(&self, _: &str) -> bool {
            false
        }

        /// The path of the socket.
        #[must_use]
        pub fn socket_path(&self) -> &Path {
            Path::new("")
        }

        /// The path of the file holding the session token.
        #[must_use]
        pub fn token_path(&self) -> &Path {
            Path::new("")
        }

        /// Always empty: there is no socket.
        #[must_use]
        pub fn client_token_path(&self, _: &str) -> PathBuf {
            PathBuf::new()
        }

        /// A handle that ends [`serve`](Self::serve).
        #[must_use]
        pub fn stopper(&self) -> SocketStop {
            SocketStop(())
        }

        /// # Errors
        ///
        /// Always `Unsupported`.
        pub fn serve(&self, server: &Server<'_>) -> io::Result<()> {
            self.serve_observed(server, &NoObserver)
        }

        /// # Errors
        ///
        /// Always `Unsupported`.
        pub fn serve_observed(
            &self,
            _server: &Server<'_>,
            _observer: &dyn ConnectionObserver,
        ) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_comparison() {
        assert!(name_ok("arc-shell.1"));
        for bad in ["", ".hidden", "Upper", "a/b", "a b"] {
            assert!(!name_ok(bad), "{bad}");
        }
        assert!(!name_ok(&"a".repeat(65)));
    }

    #[cfg(unix)]
    #[test]
    fn a_token_file_must_have_the_owner_of_its_directory() {
        assert!(same_owner(1000, 1000).is_ok());
        let error = same_owner(1000, 1001).unwrap_err();
        assert_eq!(error.code(), "cmd.socket.directory");
        assert!(format!("{:?}", error.details().get("reason")).contains("owner"));
    }

    #[cfg(unix)]
    #[test]
    fn comparison_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn disabled_is_refused_before_anything_is_created() {
        let config = SocketConfig {
            enabled: false,
            directory: PathBuf::from("/nonexistent/arc-test"),
            name: "x".into(),
            handshake_timeout: Duration::from_secs(10),
        };
        assert_eq!(
            LocalSocket::bind(&config).unwrap_err().code(),
            "cmd.socket.disabled"
        );
    }
}
