//! The Unix transport of the local socket: a Unix domain socket `<name>.sock` in the run
//! directory. The directory must be private to the user, and the socket is mode 0600 and owned
//! like its directory.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

use arc_base::Error;

use super::{Channel, SocketConfig, directory_error, io_error};
use crate::error::codes;

/// The most bytes a Unix socket path may hold; see [`super::SOCKET_PATH_LIMIT`].
const LIMIT: usize = super::SOCKET_PATH_LIMIT;

/// One accepted or connected connection.
pub(super) type Stream = UnixStream;

/// The listening Unix socket.
pub(super) struct Listener(UnixListener);

impl Listener {
    /// Binds `endpoint` (the socket file). A live socket of that name is refused; a stale socket
    /// file is replaced, with the token file of the instance that left it.
    pub(super) fn bind(config: &SocketConfig, endpoint: &Path) -> Result<Self, Error> {
        match fs::symlink_metadata(endpoint) {
            Ok(meta) if meta.file_type().is_socket() => {
                if UnixStream::connect(endpoint).is_ok() {
                    return Err(io_error("in_use"));
                }
                fs::remove_file(endpoint).map_err(|_| io_error("exists"))?;
                // The instance that left this socket behind ended without removing its token.
                let _ = fs::remove_file(config.directory.join(format!("{}.token", config.name)));
            }
            Ok(_) => return Err(io_error("exists")),
            Err(_) => {}
        }
        let listener = UnixListener::bind(endpoint).map_err(|_| io_error("bind"))?;
        // From here the socket is this call's: a failure removes it and nothing else.
        match fs::set_permissions(endpoint, fs::Permissions::from_mode(0o600)) {
            Ok(()) => Ok(Self(listener)),
            Err(_) => {
                let _ = fs::remove_file(endpoint);
                Err(io_error("bind"))
            }
        }
    }

    /// Waits for the next connection.
    pub(super) fn accept(&self) -> io::Result<Stream> {
        self.0.accept().map(|(stream, _)| stream)
    }
}

/// The socket file of a configuration, after the length check.
pub(super) fn endpoint(config: &SocketConfig) -> Result<PathBuf, Error> {
    let path = config.directory.join(format!("{}.sock", config.name));
    check_path_length(&path)?;
    Ok(path)
}

/// Refuses a socket path longer than [`LIMIT`] bytes. The path is not reported: it names the
/// user's folders, and the lengths say what is wrong.
fn check_path_length(path: &Path) -> Result<(), Error> {
    let length = path.as_os_str().len();
    if length <= LIMIT {
        return Ok(());
    }
    Err(Error::new(codes::SOCKET_PATH_TOO_LONG)
        .with_arg("length", i64::try_from(length).unwrap_or(i64::MAX))
        .with_arg("limit", i64::try_from(LIMIT).unwrap_or(i64::MAX)))
}

/// Makes sure `directory` exists and only the user can enter it.
pub(super) fn prepare_directory(directory: &Path) -> Result<(), Error> {
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

/// Removes the socket file of an endpoint.
pub(super) fn remove_endpoint(endpoint: &Path) {
    let _ = fs::remove_file(endpoint);
}

/// What a stop keeps until `serve` returns. Unix needs none: the accept is ended by the connection
/// itself, which the serving loop drops at once.
pub(super) type Held = ();

/// Connects to the socket at `endpoint` only to end its `accept`.
pub(super) fn wake(endpoint: &Path) -> Held {
    let _ = UnixStream::connect(endpoint);
}

/// Creates a token file at `path` that must not exist, readable by the owner only.
pub(super) fn create_token_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Checks that a file's owner is the owner of `directory`.
pub(super) fn owner_matches(directory: &Path, file: &fs::Metadata) -> Result<(), Error> {
    let directory_uid = fs::metadata(directory)
        .map_err(|_| directory_error("create"))?
        .uid();
    if directory_uid == file.uid() {
        Ok(())
    } else {
        Err(directory_error("owner"))
    }
}

/// Fills `out` from the operating system's random source.
pub(super) fn random_bytes(out: &mut [u8]) -> io::Result<()> {
    File::open("/dev/urandom")?.read_exact(out)
}

impl Channel for UnixStream {
    fn duplicate(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn end(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }

    fn set_read_deadline(&self, deadline: Option<Instant>) -> io::Result<()> {
        let timeout = match deadline {
            None => None,
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::from(io::ErrorKind::TimedOut));
                }
                Some(left)
            }
        };
        self.set_read_timeout(timeout)
    }

    fn read_part(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = self;
        stream.read(buf)
    }

    fn write_part(&self, buf: &[u8]) -> io::Result<usize> {
        let mut stream = self;
        stream.write(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::DirBuilder;
    use std::os::unix::fs::DirBuilderExt;

    use super::*;
    use crate::protocol::socket::{LocalSocket, SOCKET_PATH_LIMIT};

    use std::time::Duration;

    /// A path of exactly `len` bytes below `base`, built from components of at most 16 bytes, or
    /// `None` when `base` is already longer than `len` or the path cannot be made exactly that long.
    fn padded(base: &Path, len: usize) -> Option<PathBuf> {
        let mut path = base.to_path_buf();
        while path.as_os_str().len() < len {
            let room = len - path.as_os_str().len();
            if room == 1 {
                return None;
            }
            // Each component costs its name plus a separator; never leave exactly one byte over.
            let mut n = (room - 1).min(16);
            if room - 1 - n == 1 {
                n -= 1;
            }
            path.push("x".repeat(n));
        }
        (path.as_os_str().len() == len).then_some(path)
    }

    #[test]
    fn a_token_file_must_have_the_owner_of_its_directory() {
        // A file this process creates in its own directory has the directory's owner.
        let dir = std::env::temp_dir().join(format!("cs{}o", std::process::id()));
        DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let path = dir.join("t.token");
        let file = create_token_file(&path).unwrap();
        let meta = file.metadata().unwrap();
        assert!(owner_matches(&dir, &meta).is_ok());
        drop(file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_socket_path_of_exactly_the_limit_binds() {
        // The socket file is "s.sock": seven bytes after the directory and its separator.
        let base = std::env::temp_dir().join(format!("cs{}e", std::process::id()));
        let Some(dir) = padded(&base, SOCKET_PATH_LIMIT - 7) else {
            return;
        };
        DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)
            .unwrap();
        let config = SocketConfig {
            enabled: true,
            directory: dir.clone(),
            name: "s".into(),
            handshake_timeout: Duration::from_secs(10),
        };
        let socket = LocalSocket::bind(&config).unwrap();
        assert_eq!(socket.socket_path().as_os_str().len(), SOCKET_PATH_LIMIT);
        drop(socket);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_socket_path_one_byte_over_the_limit_is_refused_and_nothing_is_created() {
        let base = std::env::temp_dir().join(format!("cs{}l", std::process::id()));
        let long = padded(&base, SOCKET_PATH_LIMIT - 6)
            .unwrap_or_else(|| base.join("y".repeat(SOCKET_PATH_LIMIT)));
        let config = SocketConfig {
            enabled: true,
            directory: long.clone(),
            name: "s".into(),
            handshake_timeout: Duration::from_secs(10),
        };
        let error = LocalSocket::bind(&config).unwrap_err();
        assert_eq!(error.code(), "cmd.socket.path_too_long");
        assert!(!long.exists());
        // The arguments are the two byte counts; the path is not among them.
        let arguments = error.arguments();
        assert_eq!(arguments.len(), 2);
        let limit = i64::try_from(SOCKET_PATH_LIMIT).unwrap_or(i64::MAX);
        assert!(matches!(
            arguments.get("length"),
            Some(arc_base::error::Value::Int(length)) if *length > limit
        ));
        assert_eq!(
            arguments.get("limit"),
            Some(&arc_base::error::Value::Int(limit))
        );
    }
}
