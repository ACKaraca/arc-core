//! Single instance (CORE-APP-008): a second start hands its file arguments to the running
//! instance and exits.
//!
//! [`SingleInstance::acquire`] first tries to forward the arguments over a local channel in a
//! user-private directory. A message is one line of JSON, `{"paths":["<absolute path>", ...]}`.
//! When a running instance accepts it, the caller exits; otherwise the caller is the instance and
//! [`SingleInstance::poll`] yields the paths other starts forward. The receiving side opens them
//! through the application's normal open flow, which applies its own checks.
//!
//! Unix uses a Unix domain socket `instance.sock` in a directory that must be private to the
//! user: created with mode 0700 when missing, and refused when it is a symbolic link, has any
//! group or other permission, or has another owner than the socket this process creates in it.
//! Windows named pipes are not provided yet: there, [`SingleInstance::acquire`] reports that no
//! running instance exists, so the application starts normally.

use std::path::PathBuf;
use std::time::Duration;

use arc_base::{Error, Result};

use crate::codes;

/// The name of the socket inside the instance directory.
pub const SOCKET_NAME: &str = "instance.sock";

/// The largest message, in bytes, newline excluded.
pub const MAX_MESSAGE_BYTES: usize = 64 << 10;

/// The most paths one message carries.
pub const MAX_PATHS: usize = 64;

/// The time a forwarding start or a connection of the running instance has to finish its message.
pub const DEADLINE: Duration = Duration::from_millis(500);

/// The most connections one [`SingleInstance::poll`] accepts; the rest wait for the next poll.
#[cfg(unix)]
const MAX_ACCEPTS_PER_POLL: usize = 8;

/// What [`SingleInstance::acquire`] decided.
#[derive(Debug)]
pub enum Acquisition {
    /// No instance was running: this process is the instance and listens.
    Primary(SingleInstance),
    /// A running instance accepted the arguments: this process should exit.
    Forwarded,
}

/// The listening side of the single-instance channel of the running instance.
#[derive(Debug)]
pub struct SingleInstance {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    #[cfg(unix)]
    path: PathBuf,
}

/// The message for `paths`, one line without the newline.
fn encode(paths: &[PathBuf]) -> Result<String> {
    let mut list = Vec::with_capacity(paths.len());
    for path in paths {
        let text = path
            .to_str()
            .ok_or_else(|| Error::new(codes::INSTANCE_INVALID_PATH))?;
        list.push(serde_json::Value::from(text));
    }
    let line = serde_json::json!({ "paths": list }).to_string();
    if line.len() > MAX_MESSAGE_BYTES || paths.len() > MAX_PATHS {
        return Err(Error::new(codes::INSTANCE_MESSAGE_TOO_LARGE));
    }
    Ok(line)
}

/// The absolute paths a message names. A message that is not a JSON object with a `paths` array
/// of at most [`MAX_PATHS`] entries is `None`; entries that are not absolute paths are skipped.
#[cfg(unix)]
fn decode(line: &[u8]) -> Option<Vec<PathBuf>> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    let items = value.get("paths")?.as_array()?;
    if items.len() > MAX_PATHS {
        return None;
    }
    Some(
        items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .collect(),
    )
}

#[cfg(unix)]
mod unix {
    use std::fs::{self, DirBuilder};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use arc_base::{Error, Result};

    use super::{
        Acquisition, DEADLINE, MAX_ACCEPTS_PER_POLL, MAX_MESSAGE_BYTES, SOCKET_NAME,
        SingleInstance, decode, encode,
    };
    use crate::codes;

    /// Makes sure `dir` exists and only the user can enter it.
    fn private_directory(dir: &Path) -> Result<()> {
        let refused = |reason: &'static str| {
            Error::new(codes::INSTANCE_DIRECTORY).with_detail("reason", reason)
        };
        match fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                Err(refused("not_directory"))
            }
            Ok(meta) if meta.permissions().mode() & 0o077 != 0 => Err(refused("not_private")),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e)),
            Err(e) => Err(Error::new(codes::INSTANCE_IO).with_source(e)),
        }
    }

    /// Reads from a connection; each read waits no longer than the time left before `deadline`,
    /// so one line shares one bound.
    struct Until<'a> {
        stream: &'a UnixStream,
        deadline: Instant,
    }

    impl Read for Until<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            self.stream.set_read_timeout(Some(left))?;
            let mut stream = self.stream;
            stream.read(buf)
        }
    }

    /// One line of at most [`MAX_MESSAGE_BYTES`] before the deadline, without its newline; `None`
    /// when the line is longer, incomplete or late.
    fn read_line(stream: &UnixStream) -> Option<Vec<u8>> {
        let until = Until {
            stream,
            deadline: Instant::now() + DEADLINE,
        };
        let mut reader = BufReader::new(until);
        let mut line = Vec::new();
        // One byte past the limit is read at most, so an oversize line is recognized.
        (&mut reader)
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .ok()?;
        (line.last() == Some(&b'\n')).then(|| {
            line.pop();
            line
        })
    }

    /// Sends `line` to the instance listening at `path`. `Ok(true)` when it was accepted,
    /// `Ok(false)` when nothing listens there (a stale socket or none), `Err` otherwise.
    fn forward(path: &Path, line: &str) -> Result<bool> {
        let mut stream = match UnixStream::connect(path) {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) =>
            {
                return Ok(false);
            }
            Err(e) => return Err(Error::new(codes::INSTANCE_IO).with_source(e)),
        };
        // shortcut: std offers no timeout for connect; a listener whose backlog is full can delay
        // this call. Upgrade with a non-blocking connect when the standard library offers one.
        let deadline = Instant::now() + DEADLINE;
        let left = deadline.saturating_duration_since(Instant::now());
        stream
            .set_write_timeout(Some(left))
            .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e))?;
        let sent = stream
            .write_all(line.as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .and_then(|()| stream.shutdown(std::net::Shutdown::Write));
        match sent {
            Ok(()) => Ok(true),
            Err(_) => Err(Error::new(codes::INSTANCE_BUSY)),
        }
    }

    impl SingleInstance {
        /// Forwards `args` to a running instance of the application whose channel lives in `dir`,
        /// or becomes that instance. Each argument is made absolute before it is sent.
        ///
        /// # Errors
        /// `app.instance.directory` (not a private directory), `app.instance.io`,
        /// `app.instance.busy` (a running instance did not take the message within the deadline),
        /// `app.instance.invalid_path` (an argument is not UTF-8) and
        /// `app.instance.message_too_large`.
        pub fn acquire(dir: &Path, args: &[PathBuf]) -> Result<Acquisition> {
            let absolute: Vec<PathBuf> = args
                .iter()
                .map(|arg| {
                    std::path::absolute(arg)
                        .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e))
                })
                .collect::<Result<_>>()?;
            let line = encode(&absolute)?;
            private_directory(dir)?;
            let path = dir.join(SOCKET_NAME);
            if forward(&path, &line)? {
                return Ok(Acquisition::Forwarded);
            }
            // Nothing accepts connections here: a stale socket, which is replaced. A path that is
            // another kind of file is left alone.
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_socket() => {
                    fs::remove_file(&path)
                        .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e))?;
                }
                Ok(_) => return Err(Error::new(codes::INSTANCE_IO).with_detail("reason", "exists")),
                Err(_) => {}
            }
            let listener = match UnixListener::bind(&path) {
                Ok(listener) => listener,
                // Another start bound the socket between the check and the bind.
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                    return Err(Error::new(codes::INSTANCE_BUSY));
                }
                Err(e) => return Err(Error::new(codes::INSTANCE_IO).with_source(e)),
            };
            // The socket is this call's: its owner must be the owner of the directory it lives in.
            let owner_ok = match (fs::metadata(dir), fs::symlink_metadata(&path)) {
                (Ok(dir_meta), Ok(file_meta)) => dir_meta.uid() == file_meta.uid(),
                _ => false,
            };
            if !owner_ok {
                let _ = fs::remove_file(&path);
                return Err(Error::new(codes::INSTANCE_DIRECTORY).with_detail("reason", "owner"));
            }
            listener
                .set_nonblocking(true)
                .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o600)))
                .map_err(|e| {
                    let _ = fs::remove_file(&path);
                    Error::new(codes::INSTANCE_IO).with_source(e)
                })?;
            Ok(Acquisition::Primary(Self { listener, path }))
        }

        /// Accepts the messages waiting now (at most a few per call) and returns the absolute paths
        /// they name, in arrival order. Messages that are oversize, late, malformed or not JSON are
        /// dropped and logged without their content; the next message is unaffected.
        #[must_use]
        pub fn poll(&self) -> Vec<PathBuf> {
            let mut paths = Vec::new();
            for _ in 0..MAX_ACCEPTS_PER_POLL {
                let stream = match self.listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "single instance: accept failed");
                        break;
                    }
                };
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                match read_line(&stream).as_deref().and_then(decode) {
                    Some(received) => paths.extend(received),
                    None => tracing::warn!("single instance: message dropped"),
                }
            }
            paths
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(unix))]
impl SingleInstance {
    /// Named pipes are not provided yet, so no running instance is detected: the arguments are
    /// not forwarded and the caller becomes the instance.
    ///
    /// # Errors
    /// `app.instance.invalid_path` when an argument is not UTF-8 and cannot be sent; no other
    /// error is returned on this platform.
    pub fn acquire(_dir: &std::path::Path, args: &[PathBuf]) -> Result<Acquisition> {
        encode(args)?;
        Ok(Acquisition::Primary(Self {}))
    }

    /// Nothing is ever received on this platform.
    #[must_use]
    pub fn poll(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    mod unix_tests {
        use std::fs::{self, DirBuilder};
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::os::unix::net::{UnixListener, UnixStream};

        use super::super::*;
        use crate::testutil::dir;

        fn abs(name: &str) -> PathBuf {
            PathBuf::from(format!("/d/{name}"))
        }

        /// A fresh private directory for the channel.
        fn private(name: &str) -> PathBuf {
            let run = dir(name).join("run");
            DirBuilder::new().mode(0o700).create(&run).unwrap();
            run
        }

        fn first(run: &std::path::Path) -> SingleInstance {
            match SingleInstance::acquire(run, &[]).unwrap() {
                Acquisition::Primary(instance) => instance,
                Acquisition::Forwarded => panic!("no instance runs in a fresh directory"),
            }
        }

        #[test]
        fn a_second_start_forwards_and_the_first_receives_the_paths() {
            let run = private("inst-forward");
            let running = first(&run);
            let forwarded = SingleInstance::acquire(&run, &[abs("a.arc"), abs("b.arc")]).unwrap();
            assert!(matches!(forwarded, Acquisition::Forwarded));
            assert_eq!(running.poll(), [abs("a.arc"), abs("b.arc")]);
            assert!(running.poll().is_empty());
        }

        #[test]
        fn relative_arguments_are_sent_as_absolute_paths() {
            let run = private("inst-absolute");
            let running = first(&run);
            let _ = SingleInstance::acquire(&run, &[PathBuf::from("rel.arc")]).unwrap();
            let received = running.poll();
            assert_eq!(received.len(), 1);
            assert!(received[0].is_absolute() && received[0].ends_with("rel.arc"));
        }

        #[test]
        fn a_stale_socket_is_replaced_and_the_caller_becomes_the_instance() {
            let run = private("inst-stale");
            drop(UnixListener::bind(run.join(SOCKET_NAME)).unwrap());
            assert!(
                run.join(SOCKET_NAME).exists(),
                "the socket file was left behind"
            );
            let instance = first(&run);
            let _ = SingleInstance::acquire(&run, &[abs("next.arc")]).unwrap();
            assert_eq!(instance.poll(), [abs("next.arc")]);
        }

        #[test]
        fn oversize_and_malformed_messages_are_dropped_without_affecting_the_next() {
            let run = private("inst-hostile");
            let running = first(&run);
            let socket = run.join(SOCKET_NAME);
            let mut oversize = UnixStream::connect(&socket).unwrap();
            oversize
                .write_all(&vec![b'a'; MAX_MESSAGE_BYTES + 100])
                .unwrap();
            drop(oversize);
            let mut malformed = UnixStream::connect(&socket).unwrap();
            malformed.write_all(b"{not json\n").unwrap();
            drop(malformed);
            let _ = SingleInstance::acquire(&run, &[abs("ok.arc")]).unwrap();
            assert_eq!(running.poll(), [abs("ok.arc")]);
        }

        #[test]
        fn a_directory_open_to_other_users_is_refused() {
            let run = dir("inst-perm").join("run");
            fs::create_dir(&run).unwrap();
            fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
            let error = SingleInstance::acquire(&run, &[]).unwrap_err();
            assert_eq!(error.code(), "app.instance.directory");
            assert!(!run.join(SOCKET_NAME).exists());
        }

        #[test]
        fn a_symbolic_link_to_a_directory_is_refused() {
            let real = private("inst-link");
            let link = real.with_file_name("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let error = SingleInstance::acquire(&link, &[]).unwrap_err();
            assert_eq!(error.code(), "app.instance.directory");
        }

        #[test]
        fn decoding_skips_relative_entries_and_refuses_bad_shapes() {
            assert_eq!(
                decode(br#"{"paths":["/a.arc","b.arc"]}"#),
                Some(vec![PathBuf::from("/a.arc")])
            );
            assert_eq!(decode(br#"{"paths":3}"#), None);
            assert_eq!(decode(b"[]"), None);
            let many = format!(
                "{{\"paths\":[{}]}}",
                vec!["\"/x\""; MAX_PATHS + 1].join(",")
            );
            assert_eq!(decode(many.as_bytes()), None);
        }
    }
}
