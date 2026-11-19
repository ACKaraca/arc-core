//! The Windows transport of the local socket: a named pipe named from `<name>` and the run
//! directory (see `pipe_path`).
//!
//! The name is not the protection: it carries a hash of the user identity and the run directory
//! so that two users and two products do not share a name. The protection is the security
//! descriptor of the pipe, which grants full access to the owner only and rejects remote
//! clients. The first instance is created exclusively, so a name in use is refused. The token
//! files and the run directory are protected by the user profile's default access control only:
//! a run directory outside the user profile exposes the tokens to other users.
//!
//! Each connection has two threads. A reader moves the bytes of the pipe into a bounded channel,
//! and the session waits on that channel, so the handshake deadline and the end of a connection
//! are honored while a read is pending. A writer takes the session's chunks from a bounded
//! channel and writes them to the pipe. A write or read the session waits for gives up when the
//! connection is ended; a thread stuck inside a pipe call is detached and never joined.
//!
//! The pipe calls cannot be cancelled with the crates in use. A reader therefore stays in its
//! read until the peer sends or closes, so a connection that the server ended stays open for an
//! idle peer until the peer writes or closes. The number of live readers is capped at
//! `MAX_READERS`; an accepted client beyond the cap is closed unread.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use arc_base::Error;
use interprocess::ConnectWaitMode;
use interprocess::os::windows::ToWtf16;
use interprocess::os::windows::named_pipe::{
    PipeListener, PipeListenerOptions, PipeStream, RecvPipeStream, SendPipeStream, pipe_mode,
};
use interprocess::os::windows::security_descriptor::SecurityDescriptor;

use super::{Channel, MAX_CONNECTIONS, SocketConfig, directory_error, io_error};

/// The pipe type of every connection: a byte stream.
pub(super) type Pipe = PipeStream<pipe_mode::Bytes, pipe_mode::Bytes>;

/// What a stop keeps alive until `serve` has returned: the client connection that ends the accept.
pub(super) type Held = Option<Pipe>;

/// The owner-only descriptor of the pipe, in the security descriptor string format: a protected
/// DACL with one allow entry, full access for the owner rights.
const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";

/// The time a client waits for a pipe that is busy, and a probe for a pipe in use.
const WAIT: Duration = Duration::from_millis(500);

/// The time a read or write re-checks the end of the connection while it waits.
// shortcut: polling re-checks the end flag; a wake-up channel would avoid the wake-ups.
const POLL: Duration = Duration::from_millis(20);

/// The size of one read from the pipe.
const CHUNK: usize = 4096;

/// The chunks a connection holds in either direction before its threads wait for the session.
const QUEUED_CHUNKS: usize = 4;

/// The time a reply that has started may keep queuing after the connection is ended. A reply is
/// never cut short while the peer reads it, and a peer that stops reading cannot hold the stop
/// longer than this.
const REPLY_GRACE: Duration = Duration::from_secs(1);

/// The connection attempts a stop makes to reach the pipe of its socket.
const WAKE_ATTEMPTS: usize = 5;

/// The most live reader threads (connections whose pipe is open, whether or not they are
/// served). An accepted client beyond it is closed unread.
const MAX_READERS: usize = 2 * MAX_CONNECTIONS;

/// The name of the pipe for `name` in `directory`: `\\.\pipe\arc-<name>-<hash>`, where the hash
/// is of the user identity from `USERDOMAIN` and `USERNAME` and of the directory. Two starts of
/// one user and one directory compute the same name.
#[must_use]
pub fn pipe_path(name: &str, directory: &Path) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(
        std::env::var_os("USERDOMAIN")
            .unwrap_or_default()
            .as_encoded_bytes(),
    );
    hasher.update(b"\\");
    hasher.update(
        std::env::var_os("USERNAME")
            .unwrap_or_default()
            .as_encoded_bytes(),
    );
    hasher.update(b"\0");
    hasher.update(directory.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let hex: String = digest
        .as_bytes()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect();
    PathBuf::from(format!(r"\\.\pipe\arc-{name}-{hex}"))
}

/// The pipe name of a configuration.
pub(super) fn endpoint(config: &SocketConfig) -> Result<PathBuf, Error> {
    Ok(pipe_path(&config.name, &config.directory))
}

/// Makes sure `directory` exists. Access to it and to the token files is the user profile's
/// default access control; no mode is set here.
pub(super) fn prepare_directory(directory: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(directory) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(directory_error("not_directory")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(directory).map_err(|_| directory_error("create"))
        }
        Err(_) => Err(directory_error("create")),
    }
}

/// Connects to `path` for a moment: true when a pipe of that name answers or is busy.
fn in_use(path: &str) -> bool {
    match Pipe::connect_by_path_with_wait_mode(path, ConnectWaitMode::Timeout(WAIT)) {
        Ok(_) => true,
        Err(e) => e.kind() != io::ErrorKind::NotFound,
    }
}

/// The pipe listener of a socket, and the count of live reader threads it has started.
pub(super) struct Listener {
    pipe: PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>,
    readers: Arc<AtomicUsize>,
}

impl Listener {
    /// Creates the first instance of the pipe `endpoint` with the owner-only descriptor. A name
    /// that answers, or that another instance holds, is refused.
    pub(super) fn bind(_config: &SocketConfig, endpoint: &Path) -> Result<Self, Error> {
        let path = endpoint.to_str().ok_or_else(|| io_error("bind"))?;
        if in_use(path) {
            return Err(io_error("in_use"));
        }
        let descriptor_text = OWNER_ONLY.to_wtf_16().map_err(|_| io_error("bind"))?;
        let descriptor =
            SecurityDescriptor::deserialize(&descriptor_text).map_err(|_| io_error("bind"))?;
        let pipe = PipeListenerOptions::new()
            .path(path)
            .security_descriptor(Some(descriptor))
            .create_duplex::<pipe_mode::Bytes>()
            .map_err(|_| io_error("bind"))?;
        Ok(Self {
            pipe,
            readers: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Waits for the next client and starts its threads. A client beyond the reader cap, or one
    /// whose threads cannot start, is closed unread and reported as `ConnectionAborted`, which the
    /// serving loop treats as a connection that did not arrive.
    pub(super) fn accept(&self) -> io::Result<Stream> {
        let pipe = self.pipe.accept()?;
        if self.readers.load(Ordering::Acquire) >= MAX_READERS {
            drop(pipe);
            return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
        }
        Stream::new(pipe, &self.readers)
    }
}

/// One connection: the queue into its writer thread, the inbox its reader thread fills, and the
/// state that ends it.
struct Link {
    writes: SyncSender<Vec<u8>>,
    inbox: Mutex<Inbox>,
    ended: AtomicBool,
    deadline: Mutex<Option<Instant>>,
}

/// The bytes received and not yet read, with the chunks the reader thread has not delivered.
struct Inbox {
    chunks: Receiver<io::Result<Vec<u8>>>,
    data: Vec<u8>,
    at: usize,
    /// The peer ended the stream; no more chunks follow.
    eof: bool,
}

/// A connection of a named pipe, shared by the table and the session.
pub(super) struct Stream {
    link: Arc<Link>,
}

/// Counts a reader thread for as long as it lives.
struct ReaderSlot(Arc<AtomicUsize>);

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Stream {
    fn new(pipe: Pipe, readers: &Arc<AtomicUsize>) -> io::Result<Self> {
        let (recv, send) = pipe.split();
        let (chunks_tx, chunks_rx) = sync_channel(QUEUED_CHUNKS);
        let (writes_tx, writes_rx) = sync_channel::<Vec<u8>>(QUEUED_CHUNKS);
        readers.fetch_add(1, Ordering::AcqRel);
        let slot = ReaderSlot(Arc::clone(readers));
        // Both threads are started before the connection exists; a failure drops what started.
        thread::Builder::new()
            .name("arc-socket-read".into())
            .spawn(move || {
                let _slot = slot;
                read_pipe(recv, &chunks_tx);
            })
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionAborted))?;
        thread::Builder::new()
            .name("arc-socket-write".into())
            .spawn(move || write_pipe(send, &writes_rx))
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionAborted))?;
        Ok(Self {
            link: Arc::new(Link {
                writes: writes_tx,
                inbox: Mutex::new(Inbox {
                    chunks: chunks_rx,
                    data: Vec::new(),
                    at: 0,
                    eof: false,
                }),
                ended: AtomicBool::new(false),
                deadline: Mutex::new(None),
            }),
        })
    }
}

/// The reader thread of a connection: moves what the pipe delivers into the channel until the
/// peer ends the stream, the pipe fails or the session no longer takes chunks.
fn read_pipe(mut recv: RecvPipeStream<pipe_mode::Bytes>, chunks: &SyncSender<io::Result<Vec<u8>>>) {
    let mut buf = [0u8; CHUNK];
    loop {
        let message = match recv.read(&mut buf) {
            Ok(n) => Ok(buf.get(..n).map(<[u8]>::to_vec).unwrap_or_default()),
            Err(e) => Err(e),
        };
        let last = match &message {
            Ok(bytes) => bytes.is_empty(),
            Err(_) => true,
        };
        if chunks.send(message).is_err() || last {
            break;
        }
    }
}

/// The writer thread of a connection: writes the queued chunks in order. It ends when the
/// connection's queue is dropped or a write fails; a write stuck in the pipe is never waited for.
fn write_pipe(send: SendPipeStream<pipe_mode::Bytes>, chunks: &Receiver<Vec<u8>>) {
    let mut out = &send;
    while let Ok(chunk) = chunks.recv() {
        if out.write_all(&chunk).is_err() {
            break;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Channel for Stream {
    fn duplicate(&self) -> io::Result<Self> {
        Ok(Self {
            link: Arc::clone(&self.link),
        })
    }

    fn end(&self) {
        self.link.ended.store(true, Ordering::Release);
    }

    fn set_read_deadline(&self, deadline: Option<Instant>) -> io::Result<()> {
        *lock(&self.link.deadline) = deadline;
        Ok(())
    }

    fn read_part(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut inbox = lock(&self.link.inbox);
        loop {
            if inbox.at < inbox.data.len() {
                let available = inbox.data.get(inbox.at..).unwrap_or_default();
                let n = available.len().min(buf.len());
                if let (Some(target), Some(source)) = (buf.get_mut(..n), available.get(..n)) {
                    target.copy_from_slice(source);
                }
                inbox.at += n;
                return Ok(n);
            }
            if inbox.eof {
                return Ok(0);
            }
            if self.link.ended.load(Ordering::Acquire) {
                return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
            }
            let wait = match *lock(&self.link.deadline) {
                None => POLL,
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::Error::from(io::ErrorKind::TimedOut));
                    }
                    left.min(POLL)
                }
            };
            match inbox.chunks.recv_timeout(wait) {
                Ok(Ok(chunk)) if chunk.is_empty() => inbox.eof = true,
                Ok(Ok(chunk)) => {
                    inbox.data = chunk;
                    inbox.at = 0;
                }
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => inbox.eof = true,
            }
        }
    }

    /// Queues `buf` for the writer thread, in chunks. Before the first chunk is queued the call
    /// gives up at once when the connection is ended. After it, the rest of `buf` is queued in
    /// the same call, so a reply is never cut short by `end`, except that a queue which stays full
    /// for `REPLY_GRACE` after the end gives up with an error.
    fn write_part(&self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut queued_any = false;
        let mut ended_since: Option<Instant> = None;
        for piece in buf.chunks(CHUNK) {
            let mut chunk = piece.to_vec();
            loop {
                if self.link.ended.load(Ordering::Acquire) {
                    if !queued_any {
                        return Err(io::Error::from(io::ErrorKind::ConnectionAborted));
                    }
                    let since = *ended_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= REPLY_GRACE {
                        return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                    }
                }
                match self.link.writes.try_send(chunk) {
                    Ok(()) => {
                        queued_any = true;
                        break;
                    }
                    Err(TrySendError::Full(back)) => {
                        chunk = back;
                        thread::sleep(POLL);
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                    }
                }
            }
        }
        Ok(buf.len())
    }
}

/// Connects to the pipe of a socket so that its `accept` returns. The returned connection must be
/// kept until `serve` has returned: a connection that is closed before the accept takes it can be
/// lost.
pub(super) fn wake(endpoint: &Path) -> Held {
    let path = endpoint.to_str()?;
    // A few attempts: a connect that finds the pipe busy, or a stop that races the accept, must
    // not leave the accept waiting. Each attempt waits at most `WAIT`.
    (0..WAKE_ATTEMPTS).find_map(|_| {
        Pipe::connect_by_path_with_wait_mode(path, ConnectWaitMode::Timeout(WAIT)).ok()
    })
}

/// A pipe is removed when its last instance closes; there is no file to remove.
pub(super) fn remove_endpoint(_endpoint: &Path) {}

/// Creates a token file at `path` that must not exist. The user profile's access control
/// protects it.
pub(super) fn create_token_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Token files carry no owner check on Windows; see the module documentation of the transport.
pub(super) fn owner_matches(_directory: &Path, _file: &fs::Metadata) -> Result<(), Error> {
    Ok(())
}

/// Fills `out` from the operating system's random source, through version 4 UUIDs of 16 bytes
/// each.
pub(super) fn random_bytes(out: &mut [u8]) -> io::Result<()> {
    for chunk in out.chunks_mut(16) {
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        let n = chunk.len().min(bytes.len());
        if let (Some(target), Some(source)) = (chunk.get_mut(..n), bytes.get(..n)) {
            target.copy_from_slice(source);
        }
    }
    Ok(())
}
