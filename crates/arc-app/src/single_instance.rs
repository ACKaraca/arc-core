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
//! Windows uses the named pipe `\\.\pipe\arc-instance-<hash>` in the instance directory, named
//! from the user identity and the directory (`arc-command`'s `pipe_path`). It has the owner-only
//! security descriptor of the automation socket, and its first instance is created exclusively.
//! Before the paths are sent, the running instance proves itself: it writes a random secret to
//! `instance.secret` in the directory, sends that secret when a client connects, and the client
//! compares it with the file before it sends anything. The instance acknowledges a queued message
//! with `ok`; a start that gets no matching secret or no acknowledgment within the deadline is not
//! forwarded and returns `app.instance.busy`, so it starts as an independent instance. Each
//! connection has one reader thread: at most 16 readers are within the deadline and at most 64
//! have passed it without a message; a connection beyond those is closed unread. `poll` never
//! blocks. The directory's protection is the user profile's access control: a directory outside
//! the profile exposes the secret to other users.

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

/// The most accepted connections that wait for their message. Beyond it the oldest is dropped
/// (Unix) or the newest is closed unread (Windows, where each waiting connection has a reader).
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
    /// Messages the reader threads have completed, waiting for `poll`.
    #[cfg(windows)]
    inbox: std::sync::mpsc::Receiver<Vec<PathBuf>>,
    /// Ends the accept loop when the instance is dropped.
    #[cfg(windows)]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The pipe name, which the drop connects to so the accept loop wakes up.
    #[cfg(windows)]
    path: String,
    /// The file that holds this instance's proof secret, removed when the instance is dropped.
    #[cfg(windows)]
    secret_path: PathBuf,
    /// The accept loop, joined when the instance is dropped.
    #[cfg(windows)]
    accept: Option<std::thread::JoinHandle<()>>,
    /// The start of the current one-second window and the paths returned within it.
    #[cfg(any(unix, windows))]
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

/// The length of a proof secret: 64 hexadecimal digits, as `random_token` makes.
#[cfg(any(windows, test))]
const SECRET_LEN: usize = 64;

/// Whether `offered` proves the secret `expected` of a directory. The expected secret must be a
/// full secret; an empty or short file proves nothing.
#[cfg(any(windows, test))]
fn proven(offered: &[u8], expected: &[u8]) -> bool {
    expected.len() == SECRET_LEN
        && expected.iter().all(u8::is_ascii_hexdigit)
        && offered == expected
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
#[cfg(any(unix, windows))]
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
    use std::time::Instant;

    use arc_base::{Error, Result};

    use super::{
        Acquisition, DEADLINE, MAX_ACCEPTS_PER_POLL, MAX_MESSAGE_BYTES, MAX_PENDING, Pending,
        SOCKET_NAME, SingleInstance, decode, encode,
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
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl SingleInstance {
    /// Appends the paths of one message to `out` within the per-poll and per-second bounds.
    #[cfg(any(unix, windows))]
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

#[cfg(windows)]
mod windows {
    use std::collections::BTreeMap;
    use std::fs::{self, OpenOptions};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{SyncSender, sync_channel};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use arc_base::{Error, Result};
    use arc_command::protocol::{pipe_path, random_token};
    use interprocess::ConnectWaitMode;
    use interprocess::os::windows::ToWtf16;
    use interprocess::os::windows::named_pipe::{
        PipeListener, PipeListenerOptions, PipeStream, pipe_mode,
    };
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    use super::{
        Acquisition, DEADLINE, MAX_MESSAGE_BYTES, MAX_PENDING, SingleInstance, decode, encode,
        proven,
    };
    use crate::codes;

    /// The pipe type of the channel: a byte stream.
    type Pipe = PipeStream<pipe_mode::Bytes, pipe_mode::Bytes>;

    /// The owner-only descriptor, as the automation socket uses it.
    const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";

    /// The channel's name in `pipe_path`: `\\.\pipe\arc-instance-<hash>`.
    const CHANNEL: &str = "instance";

    /// The file of the proof secret, in the instance directory. Its protection is the user
    /// profile's access control, like the rest of the directory.
    const SECRET_NAME: &str = "instance.secret";

    /// The most readers that have held their connection past the deadline without a message. They
    /// count apart from the readers still within the deadline, which are bounded by `MAX_PENDING`.
    const MAX_ABANDONED: usize = 64;

    /// The time a start waits for each step after the connection: the proof, the write of the
    /// paths and the acknowledgment. A loaded machine must not turn a healthy primary into busy.
    const STEP_WAIT: Duration = Duration::from_secs(2);

    fn io_error(e: io::Error) -> Error {
        Error::new(codes::INSTANCE_IO).with_source(e)
    }

    /// Not forwarded: the channel was taken by something that did not prove it is the instance,
    /// or no instance took the message in time. The start then runs as an independent instance.
    fn busy() -> Error {
        Error::new(codes::INSTANCE_BUSY)
    }

    /// Reads one line of at most `MAX_MESSAGE_BYTES`, newline excluded. `None` when the line is
    /// not complete within the limit or the stream ends first.
    fn read_line<R: BufRead>(reader: &mut R) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        // One byte past the limit: a line without its newline within the limit is refused.
        let read = (&mut *reader)
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line);
        if read.is_ok() && line.last() == Some(&b'\n') {
            line.pop();
            Some(line)
        } else {
            None
        }
    }

    /// Sends `line` to the instance on `name`, after the instance proved the secret of this
    /// directory and before the message is acknowledged. `Ok(true)` when forwarded, `Ok(false)`
    /// when no channel of that name exists, and `busy` when the connection, the proof, the write or
    /// the acknowledgment does not arrive in time, or the proof is wrong. Only a proven channel
    /// receives the paths.
    fn forward(dir: &Path, name: &str, line: &str) -> Result<bool> {
        let pipe =
            match Pipe::connect_by_path_with_wait_mode(name, ConnectWaitMode::Timeout(DEADLINE)) {
                Ok(pipe) => pipe,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(busy()),
                Err(e) => return Err(io_error(e)),
            };
        let (recv, send) = pipe.split();
        // A thread takes the proof, then writes the paths it is given and reads the
        // acknowledgment. Every step is reported through a channel, so the start waits with a
        // deadline; a write that stalls keeps only this thread, which is never joined.
        let (proof_tx, proof_rx) = sync_channel::<Option<Vec<u8>>>(1);
        let (go_tx, go_rx) = sync_channel::<Vec<u8>>(1);
        let (wrote_tx, wrote_rx) = sync_channel::<bool>(1);
        let (ack_tx, ack_rx) = sync_channel::<Option<Vec<u8>>>(1);
        thread::Builder::new()
            .name("arc-instance-proof".into())
            .spawn(move || {
                let mut reader = BufReader::new(recv);
                if proof_tx.send(read_line(&mut reader)).is_err() {
                    return;
                }
                let Ok(payload) = go_rx.recv() else {
                    return;
                };
                let mut out = &send;
                let wrote = out.write_all(&payload).is_ok();
                let _ = wrote_tx.send(wrote);
                if wrote {
                    let _ = ack_tx.send(read_line(&mut reader));
                }
            })
            .map_err(io_error)?;
        let offered = proof_rx
            .recv_timeout(STEP_WAIT)
            .ok()
            .flatten()
            .ok_or_else(busy)?;
        let expected = fs::read(dir.join(SECRET_NAME)).map_err(|_| busy())?;
        if !proven(&offered, &expected) {
            return Err(busy());
        }
        let mut payload = line.as_bytes().to_vec();
        payload.push(b'\n');
        go_tx.send(payload).map_err(|_| busy())?;
        if wrote_rx.recv_timeout(STEP_WAIT) != Ok(true) {
            return Err(busy());
        }
        match ack_rx.recv_timeout(STEP_WAIT).ok().flatten() {
            Some(ack) if ack == b"ok" => Ok(true),
            _ => Err(busy()),
        }
    }

    /// Creates the first instance of the channel with the owner-only descriptor.
    fn listen(name: &str) -> io::Result<PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>> {
        let text = OWNER_ONLY
            .to_wtf_16()
            .map_err(|_| io::Error::other("descriptor"))?;
        let descriptor = SecurityDescriptor::deserialize(&text)?;
        PipeListenerOptions::new()
            .path(name)
            .security_descriptor(Some(descriptor))
            .create_duplex::<pipe_mode::Bytes>()
    }

    /// Writes the proof secret of this instance, replacing a file a stopped instance left behind.
    fn write_secret(path: &Path, secret: &str) -> io::Result<()> {
        let _ = fs::remove_file(path);
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(secret.as_bytes())
    }

    /// The readers the accept loop has started and not finished, with the time each one started.
    #[derive(Default)]
    struct Readers {
        live: Mutex<BTreeMap<u64, Instant>>,
        next: AtomicU64,
    }

    impl Readers {
        fn lock(&self) -> MutexGuard<'_, BTreeMap<u64, Instant>> {
            self.live.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Admits a new reader unless `MAX_PENDING` readers are still within the deadline or
        /// `MAX_ABANDONED` readers have passed it without a message.
        fn admit(&self, now: Instant) -> Option<u64> {
            let mut live = self.lock();
            let pending = live
                .values()
                .filter(|started| now.saturating_duration_since(**started) < DEADLINE)
                .count();
            let abandoned = live.len() - pending;
            if pending >= MAX_PENDING || abandoned >= MAX_ABANDONED {
                return None;
            }
            let id = self.next.fetch_add(1, Ordering::AcqRel);
            live.insert(id, now);
            Some(id)
        }

        fn finish(&self, id: u64) {
            self.lock().remove(&id);
        }
    }

    /// The server side of one client: it sends the proof secret, reads one message and, once the
    /// message is queued for `poll`, acknowledges it. A client that does not match or fails a
    /// step gets no acknowledgment.
    fn serve_client(pipe: Pipe, secret: &str, inbox: &SyncSender<Vec<PathBuf>>) {
        let (recv, send) = pipe.split();
        let mut out = &send;
        if writeln!(out, "{secret}").is_err() {
            return;
        }
        let mut reader = BufReader::new(recv);
        let Some(line) = read_line(&mut reader) else {
            return;
        };
        let Some(paths) = decode(&line) else {
            return;
        };
        if inbox.send(paths).is_ok() {
            let _ = out.write_all(b"ok\n");
        }
    }

    /// Accepts clients until stopped. Each admitted client gets a thread; a client beyond the
    /// reader bounds is closed unread.
    fn accept_loop(
        listener: &PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>,
        inbox: &SyncSender<Vec<PathBuf>>,
        stop: &AtomicBool,
        readers: &Arc<Readers>,
        secret: &str,
    ) {
        loop {
            let accepted = listener.accept();
            if stop.load(Ordering::Acquire) {
                break;
            }
            let Ok(pipe) = accepted else {
                break;
            };
            let Some(id) = readers.admit(Instant::now()) else {
                drop(pipe);
                continue;
            };
            let inbox = inbox.clone();
            let slot = Arc::clone(readers);
            let secret = secret.to_owned();
            let spawned = thread::Builder::new()
                .name("arc-instance-read".into())
                .spawn(move || {
                    serve_client(pipe, &secret, &inbox);
                    slot.finish(id);
                });
            if spawned.is_err() {
                readers.finish(id);
            }
        }
    }

    impl SingleInstance {
        /// Forwards `args` to a running instance whose channel lives in `dir`, or becomes that
        /// instance. Each argument is made absolute before it is sent.
        ///
        /// # Errors
        /// `app.instance.invalid_path` (an argument is not UTF-8), `app.instance.message_too_large`,
        /// `app.instance.io`, and `app.instance.busy`: a running instance did not take the message
        /// within the deadline, or the channel's owner did not prove the secret of `dir` or did not
        /// acknowledge the message. The caller then starts as an independent instance.
        pub fn acquire(dir: &Path, args: &[PathBuf]) -> Result<Acquisition> {
            let absolute: Vec<PathBuf> = args
                .iter()
                .map(|arg| std::path::absolute(arg).map_err(io_error))
                .collect::<Result<_>>()?;
            let line = encode(&absolute)?;
            fs::create_dir_all(dir).map_err(io_error)?;
            let path = pipe_path(CHANNEL, dir);
            let name = path
                .to_str()
                .ok_or_else(|| io_error(io::Error::other("path")))?;
            if forward(dir, name, &line)? {
                return Ok(Acquisition::Forwarded);
            }
            let listener = match listen(name) {
                Ok(listener) => listener,
                // Another start created the channel first: its instance takes the message.
                Err(_) => {
                    return if forward(dir, name, &line)? {
                        Ok(Acquisition::Forwarded)
                    } else {
                        Err(busy())
                    };
                }
            };
            // The secret is written before the first client is accepted, so every proof a
            // client can receive matches the file.
            let secret = random_token().map_err(io_error)?;
            let secret_path = dir.join(SECRET_NAME);
            write_secret(&secret_path, &secret).map_err(io_error)?;
            let (inbox_tx, inbox_rx) = sync_channel(MAX_PENDING);
            let stop = Arc::new(AtomicBool::new(false));
            let readers = Arc::new(Readers::default());
            let accept_stop = Arc::clone(&stop);
            let accept = thread::Builder::new()
                .name("arc-instance-accept".into())
                .spawn(move || {
                    accept_loop(&listener, &inbox_tx, &accept_stop, &readers, &secret);
                })
                .map_err(io_error)?;
            Ok(Acquisition::Primary(Self {
                inbox: inbox_rx,
                stop,
                path: name.to_owned(),
                secret_path,
                accept: Some(accept),
                window: None,
            }))
        }

        /// Returns the absolute paths of the messages that have arrived, within the per-poll and
        /// per-second bounds. It never blocks: the threads deliver complete messages through the
        /// channel.
        #[must_use]
        pub fn poll(&mut self, now: Instant) -> Vec<PathBuf> {
            let mut paths = Vec::new();
            while let Ok(received) = self.inbox.try_recv() {
                self.deliver(now, received, &mut paths);
            }
            paths
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            // Removed first, while the channel name is still held: a primary that starts after
            // this one has released the name writes its own secret, which this must not remove.
            let _ = fs::remove_file(&self.secret_path);
            self.stop.store(true, Ordering::Release);
            // The connection that wakes the accept is held until the accept loop has returned: a
            // connection closed before the accept takes it can be lost.
            let wake = Pipe::connect_by_path_with_wait_mode(
                self.path.as_str(),
                ConnectWaitMode::Timeout(DEADLINE),
            );
            if let (Ok(held), Some(accept)) = (wake, self.accept.take()) {
                let _ = accept.join();
                drop(held);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
impl SingleInstance {
    /// No local channel exists on this platform, so no running instance is detected: the arguments
    /// are not forwarded and the caller becomes the instance.
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
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use crate::testutil::dir;

    /// An absolute path on this platform, named `name`.
    #[cfg(unix)]
    fn abs(name: &str) -> PathBuf {
        PathBuf::from(format!("/d/{name}"))
    }

    #[cfg(windows)]
    fn abs(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\d\{name}"))
    }

    /// One complete message naming the absolute paths `paths`, newline included.
    fn message_for(paths: &[PathBuf]) -> String {
        let names: Vec<&str> = paths.iter().filter_map(|p| p.to_str()).collect();
        format!("{}\n", serde_json::json!({ "paths": names }))
    }

    /// A fresh directory for the channel, private to the user where the platform has modes.
    fn private(name: &str) -> PathBuf {
        let run = dir(name).join("run");
        make_private(&run);
        run
    }

    #[cfg(unix)]
    fn make_private(run: &Path) {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;
        DirBuilder::new().mode(0o700).create(run).unwrap();
    }

    #[cfg(windows)]
    fn make_private(run: &Path) {
        std::fs::create_dir(run).unwrap();
    }

    /// The address of the channel of `run`: the socket file on Unix, the pipe name on Windows.
    #[cfg(unix)]
    fn address(run: &Path) -> PathBuf {
        run.join(SOCKET_NAME)
    }

    #[cfg(windows)]
    fn address(run: &Path) -> PathBuf {
        arc_command::protocol::pipe_path("instance", run)
    }

    /// A client connected to the channel of `run`.
    #[cfg(unix)]
    fn connect(run: &Path) -> std::os::unix::net::UnixStream {
        std::os::unix::net::UnixStream::connect(address(run)).unwrap()
    }

    #[cfg(windows)]
    fn connect(
        run: &Path,
    ) -> interprocess::os::windows::named_pipe::DuplexPipeStream<
        interprocess::os::windows::named_pipe::pipe_mode::Bytes,
    > {
        interprocess::os::windows::named_pipe::PipeStream::connect_by_path(
            address(run).to_str().unwrap(),
        )
        .unwrap()
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

    fn first(run: &Path) -> SingleInstance {
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
    fn two_starts_racing_yield_one_primary_and_one_forwarded() {
        let run = private("inst-race");
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
        // The writer runs beside the polls: a pipe or socket buffer smaller than the message
        // would block a write that nobody reads. The listener drops the connection at its
        // limit, so the write may end with an error, which is the expected refusal.
        let mut oversize = connect(&run);
        let writer = std::thread::spawn(move || {
            let _ = oversize.write_all(&vec![b'a'; MAX_MESSAGE_BYTES + 100]);
        });
        while !writer.is_finished() {
            assert!(running.poll(Instant::now()).is_empty());
            std::thread::yield_now();
        }
        writer.join().unwrap();
        let mut malformed = connect(&run);
        malformed.write_all(b"{not json\n").unwrap();
        drop(malformed);
        let _ = SingleInstance::acquire(&run, &[abs("ok.arc")]).unwrap();
        assert_eq!(settle(&mut running, Instant::now()), [abs("ok.arc")]);
    }

    #[test]
    fn poll_returns_immediately_while_a_connection_stays_idle() {
        let run = private("inst-idle");
        let mut running = first(&run);
        let _idle = connect(&run);
        let base = Instant::now();
        assert!(running.poll(base).is_empty());
        assert!(running.poll(base).is_empty());
    }

    #[test]
    fn a_message_split_across_two_writes_is_delivered_after_the_second() {
        let run = private("inst-split");
        let mut running = first(&run);
        let base = Instant::now();
        let mut client = connect(&run);
        let text = message_for(&[abs("split.arc")]);
        let (head, tail) = text.split_at(text.len() - 6);
        client.write_all(head.as_bytes()).unwrap();
        assert!(running.poll(base).is_empty(), "no newline yet");
        client.write_all(tail.as_bytes()).unwrap();
        assert_eq!(settle(&mut running, base), [abs("split.arc")]);
    }

    #[test]
    fn a_message_of_one_connection_is_served_next_to_an_idle_one() {
        let run = private("inst-next");
        let mut running = first(&run);
        let _idle = connect(&run);
        let mut client = connect(&run);
        client
            .write_all(message(&["/d/next.arc"]).as_bytes())
            .unwrap();
        assert_eq!(settle(&mut running, Instant::now()), [abs("next.arc")]);
    }

    #[test]
    fn a_proof_must_be_a_full_secret_and_equal() {
        let secret = "ab".repeat(32);
        assert!(proven(secret.as_bytes(), secret.as_bytes()));
        let other = "cd".repeat(32);
        assert!(!proven(other.as_bytes(), secret.as_bytes()));
        // An empty or short expected secret proves nothing, even against an empty offer.
        assert!(!proven(b"", b""));
        assert!(!proven(b"", secret.as_bytes()));
        assert!(!proven(b"xy", b"xy"));
        assert!(!proven(secret.as_bytes(), b""));
    }

    #[test]
    fn decoding_skips_relative_entries_and_refuses_bad_shapes() {
        let kept = abs("a.arc");
        let text = serde_json::json!({ "paths": [kept, "b.arc"] }).to_string();
        assert_eq!(decode(text.as_bytes()), Some(vec![kept]));
        assert_eq!(decode(br#"{"paths":3}"#), None);
        assert_eq!(decode(b"[]"), None);
        let many = serde_json::json!({ "paths": vec!["x"; MAX_PATHS + 1] }).to_string();
        assert_eq!(decode(many.as_bytes()), None);
    }

    /// A server that takes the channel name first answers with a wrong proof. The start must not
    /// send its paths to it and must not report the message as forwarded.
    #[cfg(windows)]
    #[test]
    fn an_impostor_on_the_channel_name_receives_no_path() {
        use interprocess::os::windows::named_pipe::{PipeListenerOptions, pipe_mode};
        use std::io::{BufRead, BufReader};
        let run = private("inst-impostor");
        let name = address(&run);
        let listener = PipeListenerOptions::new()
            .path(name.to_str().unwrap())
            .create_duplex::<pipe_mode::Bytes>()
            .unwrap();
        let (line_tx, lines) = std::sync::mpsc::channel::<String>();
        thread::spawn(move || {
            let Ok(pipe) = listener.accept() else {
                return;
            };
            let (recv, mut send) = pipe.split();
            let _ = writeln!(&mut send, "not the secret");
            // Every line the start sends is reported; a reader that waits forever is left behind.
            let mut reader = BufReader::new(recv);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(n) if n > 0 => {
                        let _ = line_tx.send(line);
                    }
                    _ => break,
                }
            }
        });
        // A valid-length secret other than the impostor's: the mismatch branch is what refuses it.
        std::fs::write(run.join("instance.secret"), "ab".repeat(32)).unwrap();
        let outcome = SingleInstance::acquire(&run, &[abs("secret.arc")]);
        assert!(outcome.is_err(), "an unproven channel is not forwarded to");
        // A bounded wait: no path line may arrive.
        assert!(
            lines.recv_timeout(DEADLINE * 4).is_err(),
            "no path line reached the impostor"
        );
    }

    #[cfg(unix)]
    mod unix_only {
        use std::fs::{self, DirBuilder};
        use std::io::{Read, Write};
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::time::{Duration, Instant};

        use super::super::*;
        use super::{abs, first, message, private, settle};
        use crate::testutil::dir;
        use arc_command::protocol::SOCKET_PATH_LIMIT;

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
    }
}
