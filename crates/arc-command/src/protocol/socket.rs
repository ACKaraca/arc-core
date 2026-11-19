//! Local socket sessions (CORE-CMD-020).
//!
//! A [`LocalSocket`] listens on a Unix domain socket inside a directory that only the user can
//! enter and serves the automation protocol, unchanged, to every client that presents the
//! session token. There is no other listener. The token is random, written to a file next to
//! the socket with owner-only permissions, compared in constant time and never logged or sent.
//! The first line of a connection is the transport handshake `arc.authenticate`; a connection
//! that fails it is answered with `cmd.socket.unauthorized` alone and closed. Named pipes for
//! Windows are not provided yet: [`LocalSocket::bind`] answers `cmd.socket.unsupported` there.

use std::path::PathBuf;

use arc_base::Error;

use crate::error::codes;

/// The most simultaneous connections; further ones are closed at once.
pub const MAX_CONNECTIONS: usize = 16;

/// The longest handshake line, in bytes.
pub const MAX_HANDSHAKE_BYTES: usize = 1024;

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
    use std::collections::BTreeMap;
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{self, BufReader, Read, Write};
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use arc_base::Error;
    use serde_json::Value;

    use super::{
        MAX_CONNECTIONS, MAX_HANDSHAKE_BYTES, SocketConfig, check_enabled, constant_time_eq,
        directory_error, io_error,
    };
    use crate::error::codes;
    use crate::protocol::framing::{Frame, next_frame};
    use crate::protocol::{LogLevel, Server, response};

    /// How long a connection may take to present its token.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    /// A listening local socket with its token file. Dropping it removes both files.
    #[derive(Debug)]
    pub struct LocalSocket {
        listener: UnixListener,
        socket_path: PathBuf,
        token_path: PathBuf,
        token: String,
        stop: Arc<AtomicBool>,
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

    impl LocalSocket {
        /// Creates the private directory, the token file and the socket, in this order, and
        /// returns the socket ready to accept.
        ///
        /// # Errors
        ///
        /// `cmd.socket.disabled` when automation is not enabled; `cmd.socket.directory` (detail
        /// `reason`: `name`, `not_directory`, `not_private`, `create`); `cmd.socket.io`
        /// (`in_use` when a live socket of that name exists, `exists`, `token`, `bind`).
        pub fn bind(config: &SocketConfig) -> Result<Self, Error> {
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
                }
                Ok(_) => return Err(io_error("exists")),
                Err(_) => {}
            }
            let token = new_token().map_err(|_| io_error("token"))?;
            let _ = fs::remove_file(&token_path);
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&token_path)
                .and_then(|mut file| file.write_all(token.as_bytes()))
                .map_err(|_| io_error("token"))?;
            let bound = UnixListener::bind(&socket_path).and_then(|listener| {
                fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
                Ok(listener)
            });
            match bound {
                Ok(listener) => Ok(Self {
                    listener,
                    socket_path,
                    token_path,
                    token,
                    stop: Arc::default(),
                }),
                Err(_) => {
                    let _ = fs::remove_file(&token_path);
                    let _ = fs::remove_file(&socket_path);
                    Err(io_error("bind"))
                }
            }
        }

        /// The path of the socket.
        #[must_use]
        pub fn socket_path(&self) -> &Path {
            &self.socket_path
        }

        /// The path of the file holding the session token.
        #[must_use]
        pub fn token_path(&self) -> &Path {
            &self.token_path
        }

        /// A handle that ends [`serve`](Self::serve).
        #[must_use]
        pub fn stopper(&self) -> SocketStop {
            SocketStop {
                flag: Arc::clone(&self.stop),
                path: self.socket_path.clone(),
            }
        }

        /// Accepts connections until stopped, serving each one that presents the token as a
        /// protocol session of `server` on its own thread. Returns when stopped, after the
        /// open connections were closed and their sessions ended.
        ///
        /// # Errors
        ///
        /// An error of the listener other than an aborted or interrupted accept.
        pub fn serve(&self, server: &Server<'_>) -> io::Result<()> {
            let open: Mutex<BTreeMap<u64, UnixStream>> = Mutex::default();
            let open = &open;
            let mut result = Ok(());
            thread::scope(|scope| {
                let mut next = 0u64;
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
                    let id = next;
                    next = next.wrapping_add(1);
                    let Ok(handle) = stream.try_clone() else {
                        continue;
                    };
                    {
                        let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
                        if open.len() >= MAX_CONNECTIONS {
                            continue;
                        }
                        open.insert(id, handle);
                    }
                    let spawned = thread::Builder::new()
                        .name("arc-socket-session".into())
                        .spawn_scoped(scope, move || {
                            self.connection(server, stream);
                            open.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .remove(&id);
                        });
                    if spawned.is_err() {
                        open.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .remove(&id);
                    }
                }
                for stream in open.lock().unwrap_or_else(PoisonError::into_inner).values() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            });
            result
        }

        /// The id of a well-formed `arc.authenticate` handshake with the right token.
        fn handshake(&self, line: &[u8]) -> Option<Value> {
            let message: Value = serde_json::from_slice(line).ok()?;
            let token = message.get("params")?.get("token")?.as_str()?;
            let authentic = message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
                && message.get("method").and_then(Value::as_str) == Some("arc.authenticate")
                && constant_time_eq(token.as_bytes(), self.token.as_bytes());
            let id = message.get("id")?;
            (authentic && (id.is_string() || id.is_i64() || id.is_u64())).then(|| id.clone())
        }

        fn connection(&self, server: &Server<'_>, stream: UnixStream) {
            let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
            let Ok(read_half) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(read_half);
            let mut line = Vec::new();
            let id = match next_frame(&mut reader, MAX_HANDSHAKE_BYTES, &mut line) {
                Ok(Frame::Line) => self.handshake(&line),
                _ => None,
            };
            let mut out = &stream;
            let Some(id) = id else {
                // Nothing but the documented error, then the connection ends.
                let refusal = response(
                    &Value::Null,
                    Err(Error::new(codes::SOCKET_UNAUTHORIZED).into()),
                );
                let _ = writeln!(out, "{refusal}");
                let _ = stream.shutdown(std::net::Shutdown::Both);
                server.log.log(LogLevel::Warning, "socket.unauthorized", "");
                return;
            };
            let accepted = response(&id, Ok(serde_json::json!({})));
            if writeln!(out, "{accepted}").is_err() || stream.set_read_timeout(None).is_err() {
                return;
            }
            let _ = server.serve(reader, out);
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    impl Drop for LocalSocket {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.socket_path);
            let _ = fs::remove_file(&self.token_path);
        }
    }
}

#[cfg(not(unix))]
pub use stub::{LocalSocket, SocketStop};

#[cfg(not(unix))]
mod stub {
    use std::io;
    use std::path::Path;

    use arc_base::Error;

    use super::{SocketConfig, check_enabled};
    use crate::error::codes;
    use crate::protocol::Server;

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

        /// A handle that ends [`serve`](Self::serve).
        #[must_use]
        pub fn stopper(&self) -> SocketStop {
            SocketStop(())
        }

        /// # Errors
        ///
        /// Always `Unsupported`.
        pub fn serve(&self, _server: &Server<'_>) -> io::Result<()> {
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
        };
        assert_eq!(
            LocalSocket::bind(&config).unwrap_err().code(),
            "cmd.socket.disabled"
        );
    }
}
