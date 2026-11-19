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
//! Starts decide and bind under an exclusive lock on `instance.lock` in the same directory, so
//! two starts cannot both replace a stale socket. `poll` never blocks: it reads what has arrived
//! and keeps the rest for the next call.
//! Windows named pipes are not provided yet: there, [`SingleInstance::acquire`] reports that no
//! running instance exists, so the application starts normally.

use std::path::PathBuf;
use std::time::{Duration, Instant};

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

/// The most paths one [`SingleInstance::poll`] returns. Paths beyond it are dropped and logged
/// without their content.
pub const MAX_PATHS_PER_POLL: usize = 64;

/// The most paths returned in any one second of the clock passed to [`SingleInstance::poll`].
/// The second starts at the first poll that returns a path after the previous second ended.
pub const MAX_PATHS_PER_SECOND: usize = 256;

/// The most connections one [`SingleInstance::poll`] accepts; the rest wait for the next poll.
#[cfg(unix)]
const MAX_ACCEPTS_PER_POLL: usize = 8;

/// The most accepted connections that wait for their message. Beyond it the oldest is dropped.
#[cfg(unix)]
const MAX_PENDING: usize = 16;

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
    /// Accepted connections whose message is not complete yet, oldest first.
    #[cfg(unix)]
    pending: std::collections::VecDeque<Pending>,
    /// The start of the current one-second window and the paths returned within it.
    #[cfg(unix)]
    window: Option<(Instant, usize)>,
}

/// An accepted connection whose line is not complete yet.
#[cfg(unix)]
#[derive(Debug)]
struct Pending {
    stream: std::os::unix::net::UnixStream,
    /// The bytes read so far, without the newline once it arrived.
    received: Vec<u8>,
    /// The clock value at which the connection was accepted.
    accepted: Instant,
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
    use std::collections::VecDeque;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use arc_base::{Error, Result};

    use super::{
        Acquisition, DEADLINE, MAX_ACCEPTS_PER_POLL, MAX_MESSAGE_BYTES, MAX_PATHS_PER_POLL,
        MAX_PATHS_PER_SECOND, MAX_PENDING, Pending, SOCKET_NAME, SingleInstance, decode, encode,
    };
    use crate::codes;
    use crate::run_dir::check_socket_path;

    /// The lock file that serializes the decisions of [`SingleInstance::acquire`].
    const LOCK_NAME: &str = "instance.lock";

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

    /// Takes the exclusive lock of the instance directory. The lock is released when the returned
    /// file is dropped.
    fn lock_directory(dir: &Path) -> Result<fs::File> {
        let io = |e: io::Error| Error::new(codes::INSTANCE_IO).with_source(e);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join(LOCK_NAME))
            .map_err(io)?;
        file.lock().map_err(io)?;
        Ok(file)
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

    /// What one read of a pending connection found.
    enum Step {
        /// No complete line yet.
        Waiting,
        /// A complete line; its bytes are in `received`.
        Line,
        /// The connection ends without a usable line.
        Drop,
    }

    /// Reads what the connection has now, without waiting, and reports whether its line is
    /// complete. A line longer than [`MAX_MESSAGE_BYTES`] without its newline is dropped.
    fn fill(pending: &mut Pending) -> Step {
        let mut chunk = [0u8; 4096];
        loop {
            match pending.stream.read(&mut chunk) {
                Ok(0) => return Step::Drop,
                Ok(n) => {
                    pending.received.extend_from_slice(&chunk[..n]);
                    if let Some(at) = pending.received.iter().position(|&b| b == b'\n') {
                        pending.received.truncate(at);
                        return Step::Line;
                    }
                    if pending.received.len() > MAX_MESSAGE_BYTES {
                        return Step::Drop;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Step::Waiting,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return Step::Drop,
            }
        }
    }

    impl SingleInstance {
        /// Forwards `args` to a running instance of the application whose channel lives in `dir`,
        /// or becomes that instance. Each argument is made absolute before it is sent.
        ///
        /// The decision and the bind happen under the exclusive lock `instance.lock` in `dir`, so
        /// two starts that find the same stale socket cannot both replace it.
        ///
        /// # Errors
        /// `app.instance.path_too_long` when the socket path exceeds the platform limit (nothing is
        /// created or connected to), `app.instance.directory` (not a private directory),
        /// `app.instance.io`, `app.instance.busy` (a running instance did not take the message within the deadline),
        /// `app.instance.invalid_path` (an argument is not UTF-8) and
        /// `app.instance.message_too_large`.
        pub fn acquire(dir: &Path, args: &[PathBuf]) -> Result<Acquisition> {
            // Checked first: a path the platform cannot bind is refused before anything is created
            // or connected to, and the caller decides what to do.
            check_socket_path(&dir.join(SOCKET_NAME))?;
            let absolute: Vec<PathBuf> = args
                .iter()
                .map(|arg| {
                    std::path::absolute(arg)
                        .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e))
                })
                .collect::<Result<_>>()?;
            let line = encode(&absolute)?;
            private_directory(dir)?;
            let _lock = lock_directory(dir)?;
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
                // Another process that does not take the lock bound the socket in the meantime.
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
            Ok(Acquisition::Primary(Self {
                listener,
                path,
                pending: VecDeque::new(),
                window: None,
            }))
        }

        /// Returns the absolute paths of the messages that are complete at `now`, in arrival order.
        /// It never blocks: connections are accepted and read as far as the data has arrived, and
        /// a connection whose line is still incomplete is kept for the next call.
        ///
        /// Limits: at most eight connections are accepted per call and sixteen wait at a time (the
        /// oldest is dropped beyond that); a connection is dropped when [`DEADLINE`] has passed
        /// since it was accepted on the clock passed here, when its line exceeds 64 KiB, when it
        /// is closed before a newline or when its line is not a valid message. A call returns at
        /// most [`MAX_PATHS_PER_POLL`] paths and the clock's one-second windows return at most
        /// [`MAX_PATHS_PER_SECOND`] in total; paths beyond a bound are dropped and logged.
        #[must_use]
        pub fn poll(&mut self, now: Instant) -> Vec<PathBuf> {
            self.accept(now);
            let mut paths = Vec::new();
            let mut waiting = VecDeque::with_capacity(self.pending.len());
            while let Some(mut pending) = self.pending.pop_front() {
                if now.saturating_duration_since(pending.accepted) >= DEADLINE {
                    tracing::warn!("single instance: message dropped after its deadline");
                    continue;
                }
                match fill(&mut pending) {
                    Step::Waiting => waiting.push_back(pending),
                    Step::Drop => tracing::warn!("single instance: message dropped"),
                    Step::Line => match decode(&pending.received) {
                        Some(received) => self.deliver(now, received, &mut paths),
                        None => tracing::warn!("single instance: message dropped"),
                    },
                }
            }
            self.pending = waiting;
            paths
        }

        /// Accepts the connections waiting now, at most [`MAX_ACCEPTS_PER_POLL`], and queues them
        /// with the clock value `now`.
        fn accept(&mut self, now: Instant) {
            for _ in 0..MAX_ACCEPTS_PER_POLL {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        if stream.set_nonblocking(true).is_err() {
                            continue;
                        }
                        self.pending.push_back(Pending {
                            stream,
                            received: Vec::new(),
                            accepted: now,
                        });
                        if self.pending.len() > MAX_PENDING {
                            self.pending.pop_front();
                            tracing::warn!("single instance: oldest waiting connection dropped");
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "single instance: accept failed");
                        break;
                    }
                }
            }
        }

        /// Appends the paths of one message to `out` within the per-poll and per-second bounds.
        fn deliver(&mut self, now: Instant, received: Vec<PathBuf>, out: &mut Vec<PathBuf>) {
            let (start, used) = match self.window {
                Some((start, used))
                    if now.saturating_duration_since(start) < Duration::from_secs(1) =>
                {
                    (start, used)
                }
                _ => (now, 0),
            };
            let room = MAX_PATHS_PER_SECOND
                .saturating_sub(used)
                .min(MAX_PATHS_PER_POLL.saturating_sub(out.len()));
            let take = room.min(received.len());
            if take < received.len() {
                tracing::warn!(
                    dropped = received.len() - take,
                    "single instance: paths over the bound dropped"
                );
            }
            out.extend(received.into_iter().take(take));
            self.window = Some((start, used + take));
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
    pub fn poll(&mut self, _now: Instant) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    mod unix_tests {
        use std::fs::{self, DirBuilder};
        use std::io::{Read, Write};
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::sync::{Arc, Barrier};
        use std::thread;

        use super::super::*;
        use crate::testutil::dir;
        use arc_command::protocol::SOCKET_PATH_LIMIT;

        fn abs(name: &str) -> PathBuf {
            PathBuf::from(format!("/d/{name}"))
        }

        /// A fresh private directory for the channel.
        fn private(name: &str) -> PathBuf {
            let run = dir(name).join("run");
            DirBuilder::new().mode(0o700).create(&run).unwrap();
            run
        }

        /// Polls until something arrives: delivery of a sent message is not instant on every
        /// platform, so a test that expects paths asks again instead of assuming one poll.
        fn settle(instance: &mut SingleInstance, now: Instant) -> Vec<PathBuf> {
            for _ in 0..200_000 {
                let paths = instance.poll(now);
                if !paths.is_empty() {
                    return paths;
                }
                std::thread::yield_now();
            }
            Vec::new()
        }

        fn first(run: &std::path::Path) -> SingleInstance {
            match SingleInstance::acquire(run, &[]).unwrap() {
                Acquisition::Primary(instance) => instance,
                Acquisition::Forwarded => panic!("no instance runs in a fresh directory"),
            }
        }

        /// One complete message naming `paths`, newline included.
        fn message(paths: &[&str]) -> String {
            format!("{}\n", serde_json::json!({ "paths": paths }))
        }

        #[test]
        fn a_second_start_forwards_and_the_first_receives_the_paths() {
            let run = private("inst-forward");
            let mut running = first(&run);
            let base = Instant::now();
            let forwarded = SingleInstance::acquire(&run, &[abs("a.arc"), abs("b.arc")]).unwrap();
            assert!(matches!(forwarded, Acquisition::Forwarded));
            assert_eq!(settle(&mut running, base), [abs("a.arc"), abs("b.arc")]);
            assert!(running.poll(base).is_empty());
        }

        #[test]
        fn relative_arguments_are_sent_as_absolute_paths() {
            let run = private("inst-absolute");
            let mut running = first(&run);
            let _ = SingleInstance::acquire(&run, &[PathBuf::from("rel.arc")]).unwrap();
            let received = settle(&mut running, Instant::now());
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
            let mut instance = first(&run);
            let _ = SingleInstance::acquire(&run, &[abs("next.arc")]).unwrap();
            assert_eq!(settle(&mut instance, Instant::now()), [abs("next.arc")]);
        }

        #[test]
        fn two_starts_racing_over_a_stale_socket_yield_one_primary_and_one_forwarded() {
            let run = private("inst-race");
            drop(UnixListener::bind(run.join(SOCKET_NAME)).unwrap());
            let barrier = Arc::new(Barrier::new(2));
            let starts: Vec<_> = (0..2)
                .map(|n| {
                    let run = run.clone();
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        SingleInstance::acquire(&run, &[abs(&format!("start{n}.arc"))])
                    })
                })
                .collect();
            let outcomes: Vec<Acquisition> = starts
                .into_iter()
                .map(|start| start.join().unwrap().unwrap())
                .collect();
            let mut primaries = Vec::new();
            let mut forwarded = 0;
            for outcome in outcomes {
                match outcome {
                    Acquisition::Primary(instance) => primaries.push(instance),
                    Acquisition::Forwarded => forwarded += 1,
                }
            }
            assert_eq!(primaries.len(), 1, "exactly one start is the instance");
            assert_eq!(forwarded, 1, "exactly one start forwards");
            assert_eq!(settle(&mut primaries[0], Instant::now()).len(), 1);
        }

        #[test]
        fn oversize_and_malformed_messages_are_dropped_without_affecting_the_next() {
            let run = private("inst-hostile");
            let mut running = first(&run);
            let socket = run.join(SOCKET_NAME);
            // The writer runs beside the polls: a socket buffer smaller than the message would
            // block a write that nobody reads. The listener drops the connection at its limit,
            // so the write may end with an error, which is the expected refusal.
            let mut oversize = UnixStream::connect(&socket).unwrap();
            let writer = std::thread::spawn(move || {
                let _ = oversize.write_all(&vec![b'a'; MAX_MESSAGE_BYTES + 100]);
            });
            while !writer.is_finished() {
                assert!(running.poll(Instant::now()).is_empty());
                std::thread::yield_now();
            }
            writer.join().unwrap();
            let mut malformed = UnixStream::connect(&socket).unwrap();
            malformed.write_all(b"{not json\n").unwrap();
            drop(malformed);
            let _ = SingleInstance::acquire(&run, &[abs("ok.arc")]).unwrap();
            assert_eq!(settle(&mut running, Instant::now()), [abs("ok.arc")]);
        }

        #[test]
        fn poll_returns_immediately_while_a_connection_stays_idle() {
            let run = private("inst-idle");
            let mut running = first(&run);
            let _idle = UnixStream::connect(run.join(SOCKET_NAME)).unwrap();
            let base = Instant::now();
            assert!(running.poll(base).is_empty());
            assert!(running.poll(base).is_empty());
        }

        #[test]
        fn a_message_split_across_two_writes_is_delivered_after_the_second() {
            let run = private("inst-split");
            let mut running = first(&run);
            let base = Instant::now();
            let mut client = UnixStream::connect(run.join(SOCKET_NAME)).unwrap();
            client.write_all(b"{\"paths\":[\"/d/split").unwrap();
            assert!(running.poll(base).is_empty(), "no newline yet");
            client.write_all(b".arc\"]}\n").unwrap();
            assert_eq!(settle(&mut running, base), [abs("split.arc")]);
        }

        #[test]
        fn an_idle_connection_is_dropped_at_the_deadline_and_a_later_client_is_served() {
            let run = private("inst-deadline");
            let mut running = first(&run);
            let socket = run.join(SOCKET_NAME);
            let base = Instant::now();
            let mut idle = UnixStream::connect(&socket).unwrap();
            assert!(running.poll(base).is_empty());
            let mut client = UnixStream::connect(&socket).unwrap();
            client
                .write_all(message(&["/d/late.arc"]).as_bytes())
                .unwrap();
            let later = base + DEADLINE + Duration::from_millis(1);
            assert_eq!(settle(&mut running, later), [abs("late.arc")]);
            idle.set_nonblocking(true).unwrap();
            let mut byte = [0u8; 1];
            assert_eq!(
                idle.read(&mut byte).unwrap(),
                0,
                "the server closed the idle connection"
            );
        }

        #[test]
        fn more_connections_than_the_bound_are_handled_without_blocking() {
            let run = private("inst-flood");
            let mut running = first(&run);
            let socket = run.join(SOCKET_NAME);
            let idle: Vec<UnixStream> = (0..MAX_PENDING + 4)
                .map(|_| UnixStream::connect(&socket).unwrap())
                .collect();
            let mut valid = UnixStream::connect(&socket).unwrap();
            valid
                .write_all(message(&["/d/valid.arc"]).as_bytes())
                .unwrap();
            let base = Instant::now();
            let mut received = Vec::new();
            for _ in 0..4 {
                received.extend(running.poll(base));
            }
            assert_eq!(received, [abs("valid.arc")]);
            assert_eq!(idle.len(), MAX_PENDING + 4);
        }

        #[test]
        fn returned_paths_are_bounded_per_call_and_per_second() {
            let run = private("inst-budget");
            let mut running = first(&run);
            let socket = run.join(SOCKET_NAME);
            let names: Vec<String> = (0..MAX_PATHS_PER_POLL)
                .map(|i| format!("/d/f{i}.arc"))
                .collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let base = Instant::now();
            let mut counts = Vec::new();
            for step in 0..5u64 {
                let mut client = UnixStream::connect(&socket).unwrap();
                client.write_all(message(&refs).as_bytes()).unwrap();
                let at = base + Duration::from_millis(step);
                counts.push(running.poll(at).len());
            }
            // Four full calls fill the second; the fifth message is dropped within it.
            assert_eq!(counts, [64, 64, 64, 64, 0]);
            let mut client = UnixStream::connect(&socket).unwrap();
            client.write_all(message(&refs).as_bytes()).unwrap();
            let next_second = base + Duration::from_secs(1);
            assert_eq!(running.poll(next_second).len(), MAX_PATHS_PER_POLL);
        }

        #[test]
        fn a_socket_path_one_byte_over_the_limit_is_refused_before_anything_is_created() {
            let base = std::env::temp_dir().join(format!("il{}", std::process::id()));
            let long = crate::testutil::padded(&base, SOCKET_PATH_LIMIT - 13).unwrap();
            let error = SingleInstance::acquire(&long, &[]).unwrap_err();
            assert_eq!(error.code(), "app.instance.path_too_long");
            assert!(!long.exists());
            assert_eq!(
                error.arguments().get("length"),
                Some(&arc_base::error::Value::Int(
                    i64::try_from(SOCKET_PATH_LIMIT + 1).unwrap_or(0)
                ))
            );
        }

        #[test]
        fn a_socket_path_of_exactly_the_limit_binds() {
            let base = std::env::temp_dir().join(format!("ie{}", std::process::id()));
            let run = crate::testutil::padded(&base, SOCKET_PATH_LIMIT - 14).unwrap();
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&run)
                .unwrap();
            let instance = first(&run);
            assert!(run.join(SOCKET_NAME).exists());
            drop(instance);
            let _ = fs::remove_dir_all(&base);
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
