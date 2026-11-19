//! The recovery journal (CORE-DOC-041) and crash recovery (CORE-DOC-042).
//!
//! Between saves, committed work is protected by a per-session recovery database,
//! `<root>/recovery/<document id>/<session id>.arcrec`, where `root` is the per-user application
//! data directory the application passes. A [`RecoveryJournal`] is the commit hook
//! ([`JournalSink`]) that feeds it: on the executor it builds each transaction's record and hands
//! it through a bounded queue to its storage thread, which encodes the records and writes them in
//! group commits, at the latest [`FLUSH_INTERVAL`] after the first record of a batch and whenever
//! the queue runs empty. At start-up, [`scan`] lists the recovery databases no live process holds
//! and [`recover`] replays one onto its base.
//!
//! The format and the rules are specified in `docs/architecture/document-package.md`
//! ("Explicit save over a recovery journal") and `docs/crates/arc-doc.md` ("Recovery journal
//! and crash recovery"). A recovery database is hostile input like a package: reading it never
//! panics, is bounded by [`Limits`] and fails with stable codes.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use arc_base::id::{DocumentId, Identifier};
use arc_base::{Error, Limits, Result, SessionId, TransactionId};

use super::{Mode, Package, display};
use crate::commit::{Committed, JournalSink};
use crate::diagnostic::{Diagnostic, DiagnosticSink, Origin};
use crate::document::{Document, DocumentConfig};
use crate::error::codes;
use crate::graph::Evaluators;
use crate::load::LoadOptions;
use crate::record::TxnRecord;
use crate::registry::TypeRegistry;
use crate::schema::Severity;
use crate::snapshot::Snapshot;
use crate::validate::DocumentValidator;

/// The longest time a committed transaction waits before it is durable: no transaction committed
/// more than this before a crash is lost (plus the time of one write). The storage thread writes
/// a batch at the latest this long after its first record, and the bound is enforced: while the
/// oldest record not yet written is older than this (a slow or stalled write), a further commit
/// waits for the storage thread, so no commit is acknowledged ahead of an overdue write.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// The records the queue between the executor and the storage thread holds. When it is full, a
/// commit waits for the storage thread (back-pressure), so memory stays bounded and no record is
/// dropped while recovery protection is on.
pub const QUEUE_CAPACITY: usize = 1_024;

/// `PRAGMA application_id` of a recovery database: ASCII `ARCR`.
pub const APPLICATION_ID: u32 = 0x4152_4352;

/// The recovery database format this version writes and reads (`PRAGMA user_version`).
pub const FORMAT: u32 = 1;

/// The file extension of a recovery database.
pub const EXTENSION: &str = "arcrec";

/// The directory under the application data directory that holds the recovery databases.
pub const DIRECTORY: &str = "recovery";

const SCHEMA: &str = "CREATE TABLE arc_meta (\n  key TEXT PRIMARY KEY NOT NULL,\n  value\n) WITHOUT ROWID;\nCREATE TABLE arc_record (\n  seq INTEGER PRIMARY KEY,\n  txn_id BLOB NOT NULL UNIQUE,\n  time_unix_ms INTEGER NOT NULL,\n  record BLOB NOT NULL,\n  hash BLOB NOT NULL\n);";

const KEY_FORMAT: &str = "format_version";
const KEY_DOCUMENT: &str = "document_id";
const KEY_SESSION: &str = "session_id";
const KEY_CREATED: &str = "created_unix_ms";
const KEY_BASE_PATH: &str = "base_path";
const KEY_BASE_SIZE: &str = "base_size";
const KEY_BASE_MODIFIED: &str = "base_modified_unix_ns";
const KEY_BASE_COUNTER: &str = "base_save_counter";
const KEY_BASE_COPY: &str = "base_copy";

/// The wall clock of a recovery journal: the time of each record and the flush interval.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn unix_millis(&self) -> u64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// The fingerprint of the package file a session's changes apply to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseFile {
    /// The path as the application opened it (absolute), in its text form.
    pub path: String,
    /// The file size in bytes.
    pub size: u64,
    /// The modification time in nanoseconds since the Unix epoch, as the file system reports it.
    pub modified_unix_ns: i64,
    /// The package's `save_counter`.
    pub save_counter: u64,
}

impl BaseFile {
    /// The fingerprint of the package at `path` with `save_counter`, from the file's metadata.
    /// Fails with `doc.recovery.unavailable` when the metadata cannot be read.
    pub fn of(path: &Path, save_counter: u64) -> Result<Self> {
        let metadata = fs::metadata(path).map_err(|e| unavailable(path, e))?;
        Ok(Self {
            path: display(path),
            size: metadata.len(),
            modified_unix_ns: modified_ns(&metadata),
            save_counter,
        })
    }

    /// True when the file at [`BaseFile::path`] still has this fingerprint: the same size and
    /// modification time, and it opens as a package with the same save counter.
    #[must_use]
    pub fn unchanged(&self, limits: &Limits) -> bool {
        let path = Path::new(&self.path);
        let Ok(package) = Package::open(path, Mode::ReadOnly, limits) else {
            return false;
        };
        Self::of(path, package.metadata().save_counter).is_ok_and(|now| now == *self)
    }
}

fn modified_ns(metadata: &fs::Metadata) -> i64 {
    let Ok(time) = metadata.modified() else {
        return 0;
    };
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

/// What a recovery journal protects and where it lives.
#[derive(Clone)]
pub struct Session {
    /// The per-user application data directory; the database is
    /// `<root>/recovery/<document id>/<session id>.arcrec`.
    pub root: PathBuf,
    /// The document's identifier (`arc_meta` `document_id`; a new document generates one).
    pub document: DocumentId,
    /// This session (process run).
    pub session: SessionId,
    /// The package file the changes apply to; `None` for a document never saved, whose changes
    /// apply to an empty document.
    pub base: Option<BaseFile>,
    /// The clock of the record times and the flush interval.
    pub clock: Arc<dyn Clock>,
    /// The limits records are encoded within.
    pub limits: Limits,
    /// Where failures of the recovery database are reported (the document's diagnostics list).
    pub diagnostics: Option<Arc<dyn DiagnosticSink>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("root", &self.root)
            .field("document", &self.document)
            .field("session", &self.session)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session with the system clock, default limits and no diagnostics sink.
    #[must_use]
    pub fn new(
        root: impl Into<PathBuf>,
        document: DocumentId,
        session: SessionId,
        base: Option<BaseFile>,
    ) -> Self {
        Self {
            root: root.into(),
            document,
            session,
            base,
            clock: Arc::new(SystemClock),
            limits: Limits::DEFAULT,
            diagnostics: None,
        }
    }

    fn directory(&self) -> PathBuf {
        self.root.join(DIRECTORY).join(self.document.to_string())
    }
}

/// An encoded record: transaction, commit time and bytes.
type Row = (TransactionId, u64, Vec<u8>);

/// A record on its way to the storage thread.
enum Message {
    Record {
        record: Box<TxnRecord>,
        registry: Arc<TypeRegistry>,
        time: u64,
    },
    Flush(mpsc::Sender<bool>),
    Saved {
        base: Option<BaseFile>,
        last: Option<TransactionId>,
        copy: Option<String>,
        done: mpsc::Sender<Result<()>>,
    },
}

/// What the storage thread and the handle share.
struct Status {
    protected: AtomicBool,
    written: AtomicU64,
    /// The group commits written.
    batches: AtomicU64,
    path: PathBuf,
    diagnostics: Option<Arc<dyn DiagnosticSink>>,
    /// The commit times of the records sent and not yet settled (written or dropped), oldest
    /// first.
    pending: Mutex<VecDeque<u64>>,
    settled: Condvar,
    /// False once the storage thread has ended.
    running: AtomicBool,
    /// How many commits waited for an overdue write.
    waits: AtomicU64,
}

impl Status {
    fn new(path: PathBuf, diagnostics: Option<Arc<dyn DiagnosticSink>>) -> Self {
        Self {
            protected: AtomicBool::new(true),
            written: AtomicU64::new(0),
            batches: AtomicU64::new(0),
            path,
            diagnostics,
            pending: Mutex::new(VecDeque::new()),
            settled: Condvar::new(),
            running: AtomicBool::new(true),
            waits: AtomicU64::new(0),
        }
    }

    /// Blocks while the oldest unsettled record is at least [`FLUSH_INTERVAL`] old at `clock`,
    /// unless protection is off or the storage thread has ended; then registers a record of
    /// `time`.
    fn admit(&self, clock: &dyn Clock, time: u64) {
        let interval = u64::try_from(FLUSH_INTERVAL.as_millis()).unwrap_or(u64::MAX);
        let mut pending = lock(&self.pending);
        let mut counted = false;
        while self.protected.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire)
            && pending
                .front()
                .is_some_and(|oldest| clock.unix_millis().saturating_sub(*oldest) >= interval)
        {
            if !counted {
                counted = true;
                self.waits.fetch_add(1, Ordering::AcqRel);
            }
            // Timed, so that a clock moving on or a thread ending is noticed.
            pending = self
                .settled
                .wait_timeout(pending, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        pending.push_back(time);
    }

    /// Marks `count` records, the oldest ones, as written or dropped.
    fn settle(&self, count: usize) {
        let mut pending = lock(&self.pending);
        let count = count.min(pending.len());
        pending.drain(..count);
        drop(pending);
        self.settled.notify_all();
    }

    /// Turns recovery protection off and reports why, once.
    fn off(&self, error: Error) {
        if self.protected.swap(false, Ordering::AcqRel) {
            let error = Error::new(codes::RECOVERY_UNAVAILABLE)
                .with_arg("path", display(&self.path))
                .with_source(error);
            self.report(Severity::Error, error);
        }
    }

    fn report(&self, severity: Severity, error: Error) {
        if let Some(sink) = &self.diagnostics {
            sink.deliver(&[Diagnostic::new(
                severity,
                error,
                Vec::new(),
                Origin::Executor,
            )]);
        }
    }
}

/// The recovery journal of one open document: set it as the document's journal hook with
/// [`Document::set_journal`]. Failures of the recovery database never fail a commit: they turn
/// protection off ([`RecoveryJournal::is_protected`]) and are reported as a
/// `doc.recovery.unavailable` diagnostic. Dropping it is [`RecoveryJournal::close`].
pub struct RecoveryJournal {
    sender: Mutex<Option<mpsc::SyncSender<Message>>>,
    thread: Mutex<Option<JoinHandle<u64>>>,
    status: Arc<Status>,
    clock: Arc<dyn Clock>,
    lock: Mutex<Option<File>>,
    directory: PathBuf,
    session: SessionId,
    copies: AtomicU64,
}

impl std::fmt::Debug for RecoveryJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryJournal")
            .field("path", &self.status.path)
            .field("protected", &self.is_protected())
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unavailable(path: &Path, error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::new(codes::RECOVERY_UNAVAILABLE)
        .with_arg("path", display(path))
        .with_source(error)
}

fn invalid(path: &Path, reason: &'static str) -> Error {
    Error::new(codes::RECOVERY_INVALID)
        .with_arg("path", display(path))
        .with_detail("reason", reason)
}

fn database(directory: &Path, session: SessionId) -> PathBuf {
    directory.join(format!("{session}.{EXTENSION}"))
}

fn lock_file(database: &Path) -> PathBuf {
    database.with_extension("lock")
}

/// The base copy of a session, generation `n`.
fn copy_name(session: SessionId, n: u64) -> String {
    format!("{session}-{n}.arcbase")
}

/// Copies `from` to `directory/name` through a temporary file, flushed before the rename.
fn copy_base(from: &Path, directory: &Path, name: &str) -> io::Result<()> {
    let temporary = directory.join(format!("{name}.tmp"));
    let result = fs::copy(from, &temporary)
        // Opened for writing: a flush through a read-only handle is refused on Windows.
        .and_then(|_| {
            fs::OpenOptions::new()
                .write(true)
                .open(&temporary)?
                .sync_all()
        })
        .and_then(|()| fs::rename(&temporary, directory.join(name)));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Creates or opens the lock file at `path` and takes its exclusive operating-system lock;
/// `Ok(None)` when another process (or handle) holds it.
fn take_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

impl RecoveryJournal {
    /// Creates the recovery database of `session` and starts its storage thread: call it when a
    /// document is opened for editing or created, before its first commit. With a base file,
    /// a copy of it is kept beside the database, so that the changes can be recovered when the
    /// file changes later. Fails with `doc.recovery.unavailable` when the database cannot be
    /// created; the document is then not protected.
    pub fn start(session: Session) -> Result<Arc<Self>> {
        let from = session.base.as_ref().map(|base| PathBuf::from(&base.path));
        Self::start_seeded(session, from.as_deref(), Vec::new())
    }

    /// [`RecoveryJournal::start`] with the base copied from `from` and the records `seed`
    /// written before the storage thread starts.
    fn start_seeded(session: Session, from: Option<&Path>, seed: Vec<Row>) -> Result<Arc<Self>> {
        let directory = session.directory();
        fs::create_dir_all(&directory).map_err(|e| unavailable(&directory, e))?;
        let path = database(&directory, session.session);
        let lock_path = lock_file(&path);
        let held = take_lock(&lock_path)
            .map_err(|e| unavailable(&lock_path, e))?
            .ok_or_else(|| Error::new(codes::RECOVERY_LOCKED).with_arg("path", display(&path)))?;
        let status = Arc::new(Status::new(path.clone(), session.diagnostics.clone()));
        let copy = match from {
            Some(from) => {
                let name = copy_name(session.session, 0);
                match copy_base(from, &directory, &name) {
                    Ok(()) => Some(name),
                    Err(error) => {
                        // Recovery onto an unchanged base still works without the copy.
                        status.report(Severity::Warning, unavailable(from, error));
                        None
                    }
                }
            }
            None => None,
        };
        let conn =
            create(&path, &session, copy.as_deref(), &seed).map_err(|e| unavailable(&path, e))?;
        status.written.store(
            u64::try_from(seed.len()).unwrap_or(u64::MAX),
            Ordering::Release,
        );
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let writer = Writer {
            conn,
            status: Arc::clone(&status),
            clock: Arc::clone(&session.clock),
            limits: session.limits,
            directory: directory.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("arc-doc-recovery".into())
            .spawn(move || writer.run(&receiver))
            .map_err(|e| unavailable(&path, e))?;
        Ok(Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            thread: Mutex::new(Some(thread)),
            status,
            clock: session.clock,
            lock: Mutex::new(Some(held)),
            directory,
            session: session.session,
            copies: AtomicU64::new(1),
        }))
    }

    /// The path of the recovery database.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.status.path
    }

    /// True while recovery protection is on: every record so far reached the database or is on
    /// its way there.
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.status.protected.load(Ordering::Acquire)
    }

    /// The number of records the database holds durably, counted since start (the records a
    /// save truncated included).
    #[must_use]
    pub fn written(&self) -> u64 {
        self.status.written.load(Ordering::Acquire)
    }

    fn send(&self, message: Message) -> bool {
        let sender = lock(&self.sender).clone();
        // A full queue makes the sender wait for the storage thread: back-pressure.
        sender.is_some_and(|sender| sender.send(message).is_ok())
    }

    /// Writes every record queued so far and waits at most `timeout` for it; true when they are
    /// on disk and protection is on.
    #[must_use]
    pub fn flush(&self, timeout: Duration) -> bool {
        let (done, wait) = mpsc::channel();
        self.send(Message::Flush(done)) && wait.recv_timeout(timeout).unwrap_or(false)
    }

    /// Records a successful save: `base` is the fingerprint of the package just written, which
    /// is the document's new base, and `last` the last transaction of the saved state (`None`
    /// when the session committed nothing before it, which deletes no record). The records up to
    /// `last` are deleted (all records when the database does not hold `last`), the new base
    /// is copied and recorded, and protection is turned on again if a failure had turned it
    /// off. Waits for the storage thread. Fails with `doc.recovery.unavailable`; protection is
    /// then off.
    pub fn saved(&self, base: Option<BaseFile>, last: Option<TransactionId>) -> Result<()> {
        let copy = base.as_ref().and_then(|base| {
            let name = copy_name(self.session, self.copies.fetch_add(1, Ordering::AcqRel));
            match copy_base(Path::new(&base.path), &self.directory, &name) {
                Ok(()) => Some(name),
                Err(error) => {
                    let path = Path::new(&base.path);
                    self.status
                        .report(Severity::Warning, unavailable(path, error));
                    None
                }
            }
        });
        let (done, wait) = mpsc::channel();
        let message = Message::Saved {
            base,
            last,
            copy,
            done,
        };
        if !self.send(message) {
            return Err(
                Error::new(codes::RECOVERY_UNAVAILABLE).with_arg("path", display(self.path()))
            );
        }
        wait.recv().unwrap_or_else(|_| {
            Err(Error::new(codes::RECOVERY_UNAVAILABLE).with_arg("path", display(self.path())))
        })
    }

    /// Stops the storage thread after writing every queued record; returns the number of records
    /// the database holds, `None` when the thread was already stopped.
    fn stop(&self) -> Option<u64> {
        drop(lock(&self.sender).take());
        let thread = lock(&self.thread).take()?;
        thread.join().ok()
    }

    /// Closes the journal on a clean close of the document: when the database holds no
    /// record (nothing unsaved), it is deleted with its base copies and lock; otherwise it is
    /// kept, so that it is offered for recovery, until [`RecoveryJournal::discard`] after the
    /// user confirmed discarding the changes. Closing twice does nothing more.
    pub fn close(&self) {
        if self.stop() == Some(0) {
            self.remove();
        }
    }

    /// Stops the journal and deletes its database, base copies and lock: the user confirmed
    /// discarding the unsaved changes.
    pub fn discard(&self) {
        self.stop();
        self.remove();
    }

    fn remove(&self) {
        remove_session(&self.directory, &self.status.path, self.session);
        if let Some(file) = lock(&self.lock).take() {
            let _ = file.unlock();
            drop(file);
            let _ = fs::remove_file(lock_file(&self.status.path));
        }
    }
}

/// Removes the database at `path` with its WAL files and the base copies of `session`.
fn remove_session(directory: &Path, path: &Path, session: SessionId) {
    remove_database(path);
    let prefix = format!("{session}-");
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name.contains(".arcbase") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

impl Drop for RecoveryJournal {
    fn drop(&mut self) {
        self.close();
    }
}

impl JournalSink for RecoveryJournal {
    fn record(&self, committed: &Committed) {
        if !self.is_protected() {
            return;
        }
        let record = TxnRecord::from_commit(
            committed.meta.clone(),
            committed.ops.clone(),
            &committed.before,
        );
        match record {
            Ok(record) => {
                let time = self.clock.unix_millis();
                self.status.admit(&*self.clock, time);
                let message = Message::Record {
                    record: Box::new(record),
                    registry: Arc::clone(committed.after.registry()),
                    time,
                };
                if !self.send(message) {
                    self.status.settle(1);
                    self.status.off(
                        Error::new(codes::RECOVERY_UNAVAILABLE)
                            .with_arg("path", display(self.path())),
                    );
                }
            }
            Err(error) => self.status.off(error),
        }
    }
}

/// Creates the database at `path` with its schema, metadata and `seed` records.
fn create(
    path: &Path,
    session: &Session,
    copy: Option<&str>,
    seed: &[Row],
) -> rusqlite::Result<Connection> {
    remove_database(path);
    let mut conn = Connection::open(path)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })?;
    conn.execute_batch("PRAGMA synchronous = FULL;")?;
    let txn = conn.transaction()?;
    // Inside the transaction, so that a kill leaves either an empty database or a complete one.
    txn.execute_batch(&format!(
        "PRAGMA application_id = {}; PRAGMA user_version = {FORMAT};",
        i64::from(APPLICATION_ID)
    ))?;
    txn.execute_batch(SCHEMA)?;
    {
        let mut meta = txn.prepare("INSERT INTO arc_meta (key, value) VALUES (?1, ?2)")?;
        meta.execute(params![KEY_FORMAT, FORMAT.to_string()])?;
        meta.execute(params![KEY_DOCUMENT, session.document.to_string()])?;
        meta.execute(params![KEY_SESSION, session.session.to_string()])?;
        let created = i64::try_from(session.clock.unix_millis()).unwrap_or(i64::MAX);
        meta.execute(params![KEY_CREATED, created])?;
        if let Some(base) = &session.base {
            write_base(&txn, base)?;
        }
        if let Some(copy) = copy {
            meta.execute(params![KEY_BASE_COPY, copy])?;
        }
        let mut insert = txn.prepare(
            "INSERT INTO arc_record (txn_id, time_unix_ms, record, hash) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (id, time, bytes) in seed {
            let time = i64::try_from(*time).unwrap_or(i64::MAX);
            let hash = blake3::hash(bytes);
            insert.execute(params![
                id.as_u128().to_be_bytes().as_slice(),
                time,
                bytes,
                hash.as_bytes().as_slice()
            ])?;
        }
    }
    txn.commit()?;
    Ok(conn)
}

fn write_base(conn: &Connection, base: &BaseFile) -> rusqlite::Result<()> {
    let mut meta = conn.prepare("INSERT OR REPLACE INTO arc_meta (key, value) VALUES (?1, ?2)")?;
    meta.execute(params![KEY_BASE_PATH, base.path])?;
    meta.execute(params![
        KEY_BASE_SIZE,
        i64::try_from(base.size).unwrap_or(i64::MAX)
    ])?;
    meta.execute(params![KEY_BASE_MODIFIED, base.modified_unix_ns])?;
    let counter = i64::try_from(base.save_counter).unwrap_or(i64::MAX);
    meta.execute(params![KEY_BASE_COUNTER, counter])?;
    Ok(())
}

/// The storage thread of a recovery journal.
struct Writer {
    conn: Connection,
    status: Arc<Status>,
    clock: Arc<dyn Clock>,
    limits: Limits,
    directory: PathBuf,
}

impl Writer {
    /// Writes records until the handle closes the queue; returns the records the database
    /// holds then.
    fn run(mut self, receiver: &mpsc::Receiver<Message>) -> u64 {
        let status = Arc::clone(&self.status);
        let held = self.run_loop(receiver);
        // Whatever ended the loop, no commit waits for this thread any more.
        status.running.store(false, Ordering::Release);
        status.settled.notify_all();
        held
    }

    fn run_loop(&mut self, receiver: &mpsc::Receiver<Message>) -> u64 {
        let interval = u64::try_from(FLUSH_INTERVAL.as_millis()).unwrap_or(u64::MAX);
        let mut batch: Vec<Row> = Vec::new();
        let mut started = 0;
        loop {
            let message = if batch.is_empty() {
                match receiver.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                }
            } else {
                match receiver.try_recv() {
                    Ok(message) => message,
                    // The queue ran empty: write what is waiting.
                    Err(mpsc::TryRecvError::Empty) => {
                        self.flush(&mut batch);
                        continue;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            };
            match message {
                Message::Record {
                    record,
                    registry,
                    time,
                } => {
                    if !self.status.protected.load(Ordering::Acquire) {
                        self.status.settle(1);
                        continue;
                    }
                    match record.encode(&registry, &self.limits) {
                        Ok(bytes) => {
                            if batch.is_empty() {
                                started = self.clock.unix_millis();
                            }
                            batch.push((record.txn_id, time, bytes));
                        }
                        Err(error) => {
                            self.status.off(error);
                            self.status.settle(1);
                        }
                    }
                }
                Message::Flush(done) => {
                    self.flush(&mut batch);
                    let _ = done.send(self.status.protected.load(Ordering::Acquire));
                }
                Message::Saved {
                    base,
                    last,
                    copy,
                    done,
                } => {
                    self.flush(&mut batch);
                    let result = self.rebase(base.as_ref(), last, copy.as_deref());
                    let _ = done.send(result);
                }
            }
            if !batch.is_empty() && self.clock.unix_millis().saturating_sub(started) >= interval {
                self.flush(&mut batch);
            }
        }
        self.flush(&mut batch);
        self.conn
            .query_row("SELECT count(*) FROM arc_record", [], |row| {
                row.get::<_, i64>(0)
            })
            .ok()
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(u64::MAX)
    }

    /// Writes `batch` in one transaction (a group commit).
    fn flush(&mut self, batch: &mut Vec<Row>) {
        if batch.is_empty() {
            return;
        }
        let records = std::mem::take(batch);
        let count = u64::try_from(records.len()).unwrap_or(u64::MAX);
        let written = (|| -> rusqlite::Result<()> {
            let txn = self.conn.transaction()?;
            {
                let mut insert = txn.prepare(
                    "INSERT INTO arc_record (txn_id, time_unix_ms, record, hash) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for (id, time, bytes) in &records {
                    let time = i64::try_from(*time).unwrap_or(i64::MAX);
                    let hash = blake3::hash(bytes);
                    insert.execute(params![
                        id.as_u128().to_be_bytes().as_slice(),
                        time,
                        bytes,
                        hash.as_bytes().as_slice()
                    ])?;
                }
            }
            txn.commit()
        })();
        match written {
            Ok(()) => {
                self.status.written.fetch_add(count, Ordering::AcqRel);
                self.status.batches.fetch_add(1, Ordering::AcqRel);
            }
            Err(error) => self.status.off(unavailable(&self.status.path, error)),
        }
        self.status.settle(records.len());
    }

    /// Truncates the records to the saved state and records the new base.
    fn rebase(
        &mut self,
        base: Option<&BaseFile>,
        last: Option<TransactionId>,
        copy: Option<&str>,
    ) -> Result<()> {
        let path = self.status.path.clone();
        let old_copy: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM arc_meta WHERE key = ?1",
                [KEY_BASE_COPY],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let result = (|| -> rusqlite::Result<()> {
            let txn = self.conn.transaction()?;
            if let Some(last) = last {
                let id = last.as_u128().to_be_bytes();
                let seq: Option<i64> = txn
                    .query_row(
                        "SELECT seq FROM arc_record WHERE txn_id = ?1",
                        [id.as_slice()],
                        |row| row.get(0),
                    )
                    .optional()?;
                match seq {
                    Some(seq) => txn.execute("DELETE FROM arc_record WHERE seq <= ?1", [seq])?,
                    None => txn.execute("DELETE FROM arc_record", [])?,
                };
            }
            txn.execute(
                "DELETE FROM arc_meta WHERE key IN (?1, ?2, ?3, ?4, ?5)",
                params![
                    KEY_BASE_PATH,
                    KEY_BASE_SIZE,
                    KEY_BASE_MODIFIED,
                    KEY_BASE_COUNTER,
                    KEY_BASE_COPY
                ],
            )?;
            if let Some(base) = base {
                write_base(&txn, base)?;
            }
            if let Some(copy) = copy {
                txn.execute(
                    "INSERT INTO arc_meta (key, value) VALUES (?1, ?2)",
                    params![KEY_BASE_COPY, copy],
                )?;
            }
            txn.commit()
        })();
        match result {
            Ok(()) => {
                if let Some(old) = old_copy.filter(|old| Some(old.as_str()) != copy) {
                    let _ = fs::remove_file(self.directory.join(old));
                }
                self.status.protected.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                self.status.off(unavailable(&path, error));
                Err(Error::new(codes::RECOVERY_UNAVAILABLE).with_arg("path", display(&path)))
            }
        }
    }
}

/// A recovery database that no live process holds, found by [`scan`].
#[derive(Debug)]
pub struct Orphan {
    /// The database.
    pub path: PathBuf,
    /// The document it belongs to.
    pub document: Option<DocumentId>,
    /// The session that wrote it.
    pub session: Option<SessionId>,
    /// The package file its changes apply to; `None` for a document never saved.
    pub base: Option<BaseFile>,
    /// The number of records.
    pub records: u64,
    /// When the session began, in milliseconds since the Unix epoch.
    pub created_unix_ms: Option<u64>,
    /// When the last record was committed.
    pub last_unix_ms: Option<u64>,
    /// Why the database cannot be read, when it cannot: it can only be discarded then.
    pub problem: Option<Error>,
}

/// True when the lock file beside `database` is not held by a live process (the rule of the
/// edit lock: another process can take its exclusive lock), or does not exist.
fn orphaned(database: &Path) -> bool {
    let path = lock_file(database);
    match File::open(&path) {
        Err(error) => error.kind() == io::ErrorKind::NotFound,
        Ok(file) => match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                true
            }
            Err(_) => false,
        },
    }
}

/// Lists the recovery databases under `<root>/recovery` that no live process holds, in path
/// order, at most `limits.record_count`. A database whose creation was interrupted before its
/// tables existed is not listed. A database that cannot be read is listed with its
/// [`Orphan::problem`]. Fails with `doc.recovery.unavailable` only when the directory exists and
/// cannot be listed; a missing directory lists nothing.
pub fn scan(root: &Path, limits: &Limits) -> Result<Vec<Orphan>> {
    let top = root.join(DIRECTORY);
    let mut found = Vec::new();
    let documents = match fs::read_dir(&top) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unavailable(&top, error)),
    };
    let max = usize::try_from(limits.record_count).unwrap_or(usize::MAX);
    'outer: for document in documents.flatten() {
        let Ok(entries) = fs::read_dir(document.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
                continue;
            }
            if found.len() >= max {
                break 'outer;
            }
            if orphaned(&path) && !uninitialized(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found
        .into_iter()
        .map(|path| inspect(path, limits))
        .collect())
}

/// Whether the process died while creating the database: the file opens as a database with
/// no application id and no tables. The identification, the tables and the first rows are written in one transaction before any
/// commit is acknowledged, so such a file holds nothing to recover.
fn uninitialized(path: &Path) -> bool {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let content = Connection::open_with_flags(path, flags).and_then(|conn| {
        conn.query_row(
            "SELECT (SELECT count(*) FROM sqlite_master) + application_id \
             FROM pragma_application_id",
            [],
            |row| row.get::<_, i64>(0),
        )
    });
    match content {
        Ok(content) => content == 0,
        // SQLITE_READONLY_ROLLBACK: a hot rollback journal. The database leaves rollback-journal
        // mode before its first transaction, so the process died while switching modes.
        Err(rusqlite::Error::SqliteFailure(failure, _)) => failure.extended_code == 776,
        Err(_) => false,
    }
}

fn inspect(path: PathBuf, limits: &Limits) -> Orphan {
    let mut orphan = Orphan {
        path,
        document: None,
        session: None,
        base: None,
        records: 0,
        created_unix_ms: None,
        last_unix_ms: None,
        problem: None,
    };
    let read = (|| -> Result<()> {
        let conn = connect(&orphan.path, limits)?;
        let meta = read_meta(&conn, &orphan.path)?;
        orphan.document = meta.document;
        orphan.session = meta.session;
        orphan.base = meta.base;
        orphan.created_unix_ms = meta.created;
        let (count, last): (i64, Option<i64>) = conn
            .query_row(
                "SELECT count(*), max(time_unix_ms) FROM arc_record",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| damaged(&orphan.path, e))?;
        orphan.records = u64::try_from(count).unwrap_or(0);
        orphan.last_unix_ms = last.and_then(|t| u64::try_from(t).ok());
        Ok(())
    })();
    orphan.problem = read.err();
    orphan
}

fn damaged(path: &Path, error: rusqlite::Error) -> Error {
    Error::new(codes::RECOVERY_INVALID)
        .with_arg("path", display(path))
        .with_source(error)
}

/// Opens a recovery database read-only with the protections of a package reader and checks its
/// application id, format and tables.
fn connect(path: &Path, limits: &Limits) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| damaged(path, e))?;
    let length = i32::try_from(limits.input_bytes).unwrap_or(i32::MAX);
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, length)
        .map_err(|e| damaged(path, e))?;
    conn.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA cell_size_check = ON;")
        .map_err(|e| damaged(path, e))?;
    let pragma = |name: &str| {
        conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0))
            .map_err(|e| damaged(path, e))
    };
    if pragma("application_id")? != i64::from(APPLICATION_ID) {
        return Err(invalid(path, "application_id"));
    }
    if pragma("user_version")? != i64::from(FORMAT) {
        return Err(invalid(path, "user_version"));
    }
    for (table, columns) in [
        ("arc_meta", &["key", "value"][..]),
        (
            "arc_record",
            &["seq", "txn_id", "time_unix_ms", "record", "hash"][..],
        ),
    ] {
        let mut statement = conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(|e| damaged(path, e))?;
        let present: Vec<String> = statement
            .query_map([table], |row| row.get(0))
            .and_then(Iterator::collect)
            .map_err(|e| damaged(path, e))?;
        if columns.iter().any(|c| !present.iter().any(|p| p == c)) {
            return Err(invalid(path, "schema"));
        }
    }
    Ok(conn)
}

struct Meta {
    document: Option<DocumentId>,
    session: Option<SessionId>,
    base: Option<BaseFile>,
    created: Option<u64>,
    copy: Option<String>,
}

fn read_meta(conn: &Connection, path: &Path) -> Result<Meta> {
    let get = |key: &str| -> Result<Option<ValueOwned>> {
        conn.query_row("SELECT value FROM arc_meta WHERE key = ?1", [key], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Integer(n) => ValueOwned::Integer(n),
                ValueRef::Text(text) => {
                    ValueOwned::Text(String::from_utf8_lossy(text).into_owned())
                }
                _ => ValueOwned::Other,
            })
        })
        .optional()
        .map_err(|e| damaged(path, e))
    };
    let text = |key: &str| -> Result<Option<String>> {
        match get(key)? {
            None => Ok(None),
            Some(ValueOwned::Text(text)) => Ok(Some(text)),
            Some(_) => Err(invalid(path, "metadata")),
        }
    };
    let int = |key: &str| -> Result<Option<i64>> {
        match get(key)? {
            None => Ok(None),
            Some(ValueOwned::Integer(n)) => Ok(Some(n)),
            Some(_) => Err(invalid(path, "metadata")),
        }
    };
    if text(KEY_FORMAT)?.as_deref() != Some("1") {
        return Err(invalid(path, "format_version"));
    }
    let document = text(KEY_DOCUMENT)?
        .map(|t| DocumentId::from_str(&t).map_err(|_| invalid(path, "document_id")))
        .transpose()?;
    let session = text(KEY_SESSION)?
        .map(|t| SessionId::from_str(&t).map_err(|_| invalid(path, "session_id")))
        .transpose()?;
    let base = match text(KEY_BASE_PATH)? {
        None => None,
        Some(base_path) => {
            let size = int(KEY_BASE_SIZE)?.and_then(|n| u64::try_from(n).ok());
            let modified = int(KEY_BASE_MODIFIED)?;
            let counter = int(KEY_BASE_COUNTER)?.and_then(|n| u64::try_from(n).ok());
            match (size, modified, counter) {
                (Some(size), Some(modified_unix_ns), Some(save_counter)) => Some(BaseFile {
                    path: base_path,
                    size,
                    modified_unix_ns,
                    save_counter,
                }),
                _ => return Err(invalid(path, "base")),
            }
        }
    };
    let created = int(KEY_CREATED)?.and_then(|n| u64::try_from(n).ok());
    let copy = text(KEY_BASE_COPY)?;
    // The copy is a file name in the database's directory, never a path.
    if copy
        .as_deref()
        .is_some_and(|c| c.is_empty() || c.contains(['/', '\\']) || c.contains(".."))
    {
        return Err(invalid(path, "base_copy"));
    }
    Ok(Meta {
        document,
        session,
        base,
        created,
        copy,
    })
}

enum ValueOwned {
    Integer(i64),
    Text(String),
    Other,
}

/// Deletes an orphan's database, its base copies and its lock file: the user declined
/// recovery. Fails with `doc.recovery.locked` when a live process holds it.
pub fn discard(orphan: &Orphan) -> Result<()> {
    let lock_path = lock_file(&orphan.path);
    let held = take_lock(&lock_path)
        .map_err(|e| unavailable(&lock_path, e))?
        .ok_or_else(|| {
            Error::new(codes::RECOVERY_LOCKED).with_arg("path", display(&orphan.path))
        })?;
    remove_orphan(&orphan.path, orphan.session, held);
    Ok(())
}

/// Removes the database at `path`, the base copies of `session` and the lock file, releasing
/// `held` first.
fn remove_orphan(path: &Path, session: Option<SessionId>, held: File) {
    let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    match session {
        Some(session) => remove_session(&directory, path, session),
        None => remove_database(path),
    }
    let _ = held.unlock();
    drop(held);
    let _ = fs::remove_file(lock_file(path));
}

/// Removes the database at `path` with its WAL files.
fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

/// The settings of [`recover`].
pub struct RecoverOptions {
    /// The application data directory of the new session's recovery journal.
    pub root: PathBuf,
    /// The new session.
    pub session: SessionId,
    /// The configuration of the recovered document.
    pub config: DocumentConfig,
    /// The options of loading the base and the limits of reading the recovery database.
    pub load: LoadOptions,
    /// The document validators, which run at every replayed commit.
    pub validators: Vec<DocumentValidator>,
    /// The clock of the new recovery journal.
    pub clock: Arc<dyn Clock>,
    /// Where the new recovery journal reports failures.
    pub diagnostics: Option<Arc<dyn DiagnosticSink>>,
}

impl RecoverOptions {
    /// Options for a new session `session` under `root` with defaults otherwise.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, session: SessionId) -> Self {
        Self {
            root: root.into(),
            session,
            config: DocumentConfig::default(),
            load: LoadOptions::default(),
            validators: Vec::new(),
            clock: Arc::new(SystemClock),
            diagnostics: None,
        }
    }
}

/// The result of [`recover`].
#[derive(Debug)]
pub struct Recovered {
    /// The recovered document, with every replayed record in its journal; its undo stacks start
    /// empty.
    pub document: Document,
    /// The recovery journal of the new session, already set as the document's journal hook. It
    /// holds the base and the replayed records, so a crash after recovery recovers again.
    pub journal: Arc<RecoveryJournal>,
    /// True when the base file changed: the recovered state is a new unsaved document built on
    /// the copy of the base, and is saved with "save as".
    pub base_changed: bool,
    /// The number of records replayed.
    pub replayed: u64,
    /// Why the replay ended before the last record (`doc.recovery.truncated`, the failure as
    /// its source); `None` when every record was replayed.
    pub stopped: Option<Error>,
}

/// Recovers `orphan`: loads its base, replays its records in order through the normal
/// transaction path and returns the document in the recovered state.
///
/// The base is the package file when it is unchanged ([`BaseFile::unchanged`]); otherwise the
/// copy kept beside the database, and the result is a new document ([`Recovered::base_changed`]);
/// a document never saved starts empty. Each record is decoded under the limits of
/// `options.load`, must name the previous record as its parent, and is applied strictly and
/// committed with validation; the first record that fails ends the replay, so the longest valid
/// prefix is recovered and the failure is reported in [`Recovered::stopped`]. Then a recovery
/// journal of the new session is started with the same base and the replayed records, and the
/// orphan is deleted.
///
/// Fails, keeping the orphan, with `doc.recovery.locked` when a live process holds it,
/// `doc.recovery.invalid` when it is not a readable recovery database,
/// `doc.recovery.base_missing` when the base changed and no copy exists, the load and package
/// errors of the base, and `doc.recovery.unavailable` when the new journal cannot be created.
pub fn recover(
    orphan: &Orphan,
    registry: Arc<TypeRegistry>,
    evaluators: Arc<Evaluators>,
    options: RecoverOptions,
) -> Result<Recovered> {
    let lock_path = lock_file(&orphan.path);
    let held = take_lock(&lock_path)
        .map_err(|e| unavailable(&lock_path, e))?
        .ok_or_else(|| {
            Error::new(codes::RECOVERY_LOCKED).with_arg("path", display(&orphan.path))
        })?;
    let limits = options.load.limits;
    let conn = connect(&orphan.path, &limits)?;
    let meta = read_meta(&conn, &orphan.path)?;
    let document_id = meta
        .document
        .ok_or_else(|| invalid(&orphan.path, "document_id"))?;
    let directory = orphan
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let copy = meta.copy.as_ref().map(|name| directory.join(name));
    let (base_changed, source) = match &meta.base {
        None => (false, None),
        Some(base) if base.unchanged(&limits) => (false, Some(PathBuf::from(&base.path))),
        Some(base) => match copy.as_ref().filter(|copy| copy.exists()) {
            Some(copy) => (true, Some(copy.clone())),
            None => {
                return Err(
                    Error::new(codes::RECOVERY_BASE_MISSING).with_arg("path", base.path.clone())
                );
            }
        },
    };
    let document = match &source {
        Some(path) => {
            let loaded = Package::open(path, Mode::ReadOnly, &limits)?
                .load(Arc::clone(&registry), &options.load)?;
            Document::from_loaded_with(loaded, evaluators, options.config)?
        }
        None => Document::open(Snapshot::empty(registry), evaluators, options.config)?,
    };
    for validator in options.validators {
        document.add_validator(validator);
    }
    let (seed, stopped) = replay(&conn, &orphan.path, &document, &limits);
    drop(conn);
    let replayed = u64::try_from(seed.len()).unwrap_or(u64::MAX);
    // The new session copies the base from the old copy when it exists: it is the base the
    // records apply to, whether or not the file changed since.
    let from = copy
        .filter(|copy| copy.exists())
        .or_else(|| meta.base.as_ref().map(|base| PathBuf::from(&base.path)));
    let session = Session {
        root: options.root,
        document: document_id,
        session: options.session,
        base: meta.base.clone(),
        clock: options.clock,
        limits,
        diagnostics: options.diagnostics,
    };
    let journal = RecoveryJournal::start_seeded(session, from.as_deref(), seed)?;
    document.set_journal(Arc::clone(&journal) as Arc<dyn JournalSink>);
    remove_orphan(&orphan.path, meta.session, held);
    Ok(Recovered {
        document,
        journal,
        base_changed,
        replayed,
        stopped,
    })
}

/// Replays the records of `conn` onto `document` in order; returns the replayed records (for
/// the new journal) and why the replay stopped early.
fn replay(
    conn: &Connection,
    path: &Path,
    document: &Document,
    limits: &Limits,
) -> (Vec<Row>, Option<Error>) {
    let mut done = Vec::new();
    let registry = document.registry();
    let stop = |seq: i64, error: Error| {
        Error::new(codes::RECOVERY_TRUNCATED)
            .with_arg("seq", seq.to_string())
            .with_source(error)
    };
    let mut statement =
        match conn.prepare("SELECT seq, time_unix_ms, record, hash FROM arc_record ORDER BY seq") {
            Ok(statement) => statement,
            Err(error) => return (done, Some(stop(0, damaged(path, error)))),
        };
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(error) => return (done, Some(stop(0, damaged(path, error)))),
    };
    let mut last: Option<TransactionId> = None;
    let mut seq: i64 = 0;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => return (done, None),
            Err(error) => {
                return (
                    done,
                    Some(stop(seq.saturating_add(1), damaged(path, error))),
                );
            }
        };
        if u64::try_from(done.len()).unwrap_or(u64::MAX) >= limits.record_count {
            let error = Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", "record_count");
            return (done, Some(stop(seq.saturating_add(1), error)));
        }
        let read = (|| -> rusqlite::Result<(i64, i64, Vec<u8>, Vec<u8>)> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })();
        let (row_seq, time, bytes, hash) = match read {
            Ok(values) => values,
            Err(error) => {
                return (
                    done,
                    Some(stop(seq.saturating_add(1), damaged(path, error))),
                );
            }
        };
        seq = row_seq;
        if blake3::hash(&bytes).as_bytes().as_slice() != hash {
            return (done, Some(stop(seq, invalid(path, "hash"))));
        }
        let record = match TxnRecord::decode(&bytes, &registry, limits) {
            Ok(record) => record,
            Err(error) => return (done, Some(stop(seq, error))),
        };
        if last.is_some() && record.parent != last {
            return (done, Some(stop(seq, invalid(path, "parent"))));
        }
        let id = record.txn_id;
        if let Err(error) = document.replay(record).wait() {
            return (done, Some(stop(seq, error)));
        }
        last = Some(id);
        done.push((id, u64::try_from(time).unwrap_or(0), bytes));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use arc_base::id::Identifier;

    use super::*;
    use crate::txn::TxnMeta;

    /// A clock that advances `step` milliseconds at every reading.
    struct Stepping(AtomicU64, u64);

    impl Clock for Stepping {
        fn unix_millis(&self) -> u64 {
            self.0.fetch_add(self.1, Ordering::Relaxed)
        }
    }

    /// Writes 10 records that are all queued before the storage thread runs, so the queue is
    /// never idle in between, and returns the number of group commits.
    fn batches(step: u64) -> u64 {
        let dir = std::env::temp_dir().join(format!(
            "arc-doc-recovery-unit-{}-{step}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let clock: Arc<dyn Clock> = Arc::new(Stepping(AtomicU64::new(0), step));
        let mut session = Session::new(
            &dir,
            DocumentId::from_u128(1),
            SessionId::from_u128(1),
            None,
        );
        session.clock = Arc::clone(&clock);
        let path = dir.join("db.arcrec");
        let conn = create(&path, &session, None, &[]).unwrap();
        let status = Arc::new(Status::new(path, None));
        let registry = Arc::new(TypeRegistry::new());
        let snapshot = Snapshot::empty(Arc::clone(&registry));
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        for n in 1..=10 {
            let meta = TxnMeta::new(TransactionId::from_u128(n), SessionId::from_u128(1));
            let record = TxnRecord::from_commit(meta, Vec::new(), &snapshot).unwrap();
            let message = Message::Record {
                record: Box::new(record),
                registry: Arc::clone(&registry),
                time: 0,
            };
            sender.send(message).unwrap();
        }
        drop(sender);
        let writer = Writer {
            conn,
            status: Arc::clone(&status),
            clock,
            limits: Limits::DEFAULT,
            directory: dir.clone(),
        };
        assert_eq!(writer.run(&receiver), 10);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(status.written.load(Ordering::Acquire), 10);
        status.batches.load(Ordering::Acquire)
    }

    #[test]
    fn a_busy_queue_is_written_at_least_once_per_flush_interval() {
        // A clock standing still: one group commit when the queue ends.
        assert_eq!(batches(0), 1);
        // A clock that passes the interval at every reading: every record is written at once.
        let interval = u64::try_from(FLUSH_INTERVAL.as_millis()).unwrap();
        assert_eq!(batches(interval), 10);
    }

    /// A clock the test sets, which parks the storage thread (a slow write) while `trap` is set.
    struct Trapped {
        now: AtomicU64,
        trap: Mutex<bool>,
        released: Condvar,
    }

    impl Trapped {
        fn trap(&self, on: bool) {
            *lock(&self.trap) = on;
            self.released.notify_all();
        }
    }

    impl Clock for Trapped {
        fn unix_millis(&self) -> u64 {
            if std::thread::current().name() == Some("arc-doc-recovery") {
                let mut trap = lock(&self.trap);
                while *trap {
                    trap = self
                        .released
                        .wait(trap)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            self.now.load(Ordering::Acquire)
        }
    }

    fn committed(n: u128) -> Committed {
        let snapshot = Snapshot::empty(Arc::new(TypeRegistry::new()));
        Committed {
            meta: TxnMeta::new(TransactionId::from_u128(n), SessionId::from_u128(1)),
            ops: Vec::new(),
            before: snapshot.clone(),
            after: snapshot,
        }
    }

    /// While the oldest unwritten record is older than the flush interval (the write is slow), a
    /// further commit waits instead of being acknowledged ahead of it.
    #[test]
    fn a_commit_waits_while_the_oldest_unwritten_record_is_overdue() {
        let dir =
            std::env::temp_dir().join(format!("arc-doc-recovery-gate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let clock = Arc::new(Trapped {
            now: AtomicU64::new(0),
            trap: Mutex::new(false),
            released: Condvar::new(),
        });
        let mut session = Session::new(
            &dir,
            DocumentId::from_u128(1),
            SessionId::from_u128(1),
            None,
        );
        session.clock = Arc::clone(&clock) as Arc<dyn Clock>;
        let journal = RecoveryJournal::start(session).unwrap();
        clock.trap(true);
        journal.record(&committed(1));
        let done = Arc::new(AtomicBool::new(false));
        clock.now.store(
            2 * u64::try_from(FLUSH_INTERVAL.as_millis()).unwrap(),
            Ordering::Release,
        );
        let second = {
            let (journal, done) = (Arc::clone(&journal), Arc::clone(&done));
            std::thread::spawn(move || {
                journal.record(&committed(2));
                done.store(true, Ordering::Release);
            })
        };
        while journal.status.waits.load(Ordering::Acquire) == 0 && !done.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let acknowledged_early = done.load(Ordering::Acquire);
        clock.trap(false);
        second.join().unwrap();
        assert!(
            !acknowledged_early,
            "acknowledged while the first write was overdue"
        );
        assert!(journal.flush(Duration::from_secs(30)));
        assert_eq!(journal.written(), 2);
        drop(journal);
        let _ = fs::remove_dir_all(&dir);
    }
}
