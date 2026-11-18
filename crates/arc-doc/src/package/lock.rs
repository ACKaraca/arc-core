//! The sidecar edit lock (CORE-DOC-046): `~<file name>.lock` beside the package, created with
//! exclusive-create semantics, holding JSON that names the holder, and held with a shared
//! operating-system lock while the package is open for editing.
//!
//! Whether a holder is alive is decided by the operating-system lock: a prober that can take an
//! exclusive lock on the file knows no process holds it. Such a lock is stale only when its host
//! is the prober's own; a lock from another host is never stale, because advisory locks are not
//! reliable across network file systems.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use arc_base::{Error, Result};

use super::ReadOnlyCause;
use super::read::io_error;
use crate::error::codes;

/// The lock content format this version writes.
const LOCK_FORMAT: u32 = 1;
/// The largest lock file this version reads.
const MAX_LOCK_BYTES: u64 = 64 * 1024;

/// Who opens a package for editing; the application supplies it because the crate makes no
/// environment queries besides its own process id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOwner {
    /// The user name.
    pub user: String,
    /// The name of this host, as the application determines it; compared byte for byte.
    pub host: String,
    /// The application and its version.
    pub application: String,
    /// The current time in milliseconds since the Unix epoch.
    pub time_unix_ms: u64,
}

/// The holder of an edit lock, as its lock file names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockHolder {
    /// The user name.
    pub user: String,
    /// The host name.
    pub host: String,
    /// The process id on that host.
    pub pid: u32,
    /// The application and its version.
    pub application: String,
    /// When the lock was taken, in milliseconds since the Unix epoch.
    pub time_unix_ms: u64,
}

/// The JSON content of a lock file.
#[derive(Serialize, Deserialize)]
struct LockFile {
    format: u32,
    #[serde(flatten)]
    holder: LockHolder,
}

/// The lock file of the package at `package`: `~<file name>.lock` in the same directory.
#[must_use]
pub fn lock_path(package: &Path) -> PathBuf {
    let name = package
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    package.with_file_name(format!("~{name}.lock"))
}

/// A lock this process holds: the open lock file with its shared lock. Dropping it releases the
/// lock and removes the file.
#[derive(Debug)]
pub(crate) struct Held {
    file: Option<File>,
    path: PathBuf,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            // Closing the file releases its lock; the file is removed afterwards so that no
            // handle of ours keeps it.
            let _ = file.unlock();
            drop(file);
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// What a probe of an existing lock file found.
enum Probe {
    /// No lock file.
    Free,
    /// A live lock, or one that cannot be judged.
    Live(Option<LockHolder>),
    /// A lock of this host that no process holds.
    Stale(LockHolder),
}

fn holder(owner: &LockOwner) -> LockHolder {
    LockHolder {
        user: owner.user.clone(),
        host: owner.host.clone(),
        pid: std::process::id(),
        application: owner.application.clone(),
        time_unix_ms: owner.time_unix_ms,
    }
}

/// Creates the lock file exclusively, writes the holder, flushes and takes the shared lock.
/// `Ok(None)` when the file already exists.
fn create(path: &Path, owner: &LockOwner) -> Result<Option<Held>> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(io_error(error, path)),
    };
    // The file is ours from here on: a failure removes it again.
    let mut held = Held {
        file: None,
        path: path.to_path_buf(),
    };
    let content = LockFile {
        format: LOCK_FORMAT,
        holder: holder(owner),
    };
    let json = serde_json::to_vec(&content).map_err(|e| {
        Error::new(codes::PACKAGE_IO_FAILED)
            .with_arg("path", path.display().to_string())
            .with_source(e)
    })?;
    let written = file
        .write_all(&json)
        .and_then(|()| file.sync_all())
        // Shared, so that probes can still read the holder on every platform.
        .and_then(|()| file.lock_shared());
    held.file = Some(file);
    match written {
        Ok(()) => Ok(Some(held)),
        Err(error) => Err(io_error(error, path)),
    }
}

/// Reads and judges an existing lock file.
fn probe(path: &Path, host: &str) -> Result<Probe> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Probe::Free),
        Err(error) => return Err(io_error(error, path)),
    };
    let mut bytes = Vec::new();
    let holder = match (&mut file).take(MAX_LOCK_BYTES).read_to_end(&mut bytes) {
        Ok(_) => serde_json::from_slice::<LockFile>(&bytes)
            .ok()
            .map(|content| content.holder),
        Err(_) => None,
    };
    let unheld = match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            true
        }
        Err(TryLockError::WouldBlock | TryLockError::Error(_)) => false,
    };
    Ok(match holder {
        // A half-written or unreadable lock may belong to an opener that is starting up.
        Some(holder) if unheld && holder.host == host => Probe::Stale(holder),
        holder => Probe::Live(holder),
    })
}

/// Takes the edit lock of `package`, or returns why the package opens read-only.
pub(crate) fn acquire(
    package: &Path,
    owner: &LockOwner,
) -> Result<std::result::Result<Held, ReadOnlyCause>> {
    let path = lock_path(package);
    // A lock removed between the create and the probe is retried once.
    for _ in 0..2 {
        if let Some(held) = create(&path, owner)? {
            return Ok(Ok(held));
        }
        match probe(&path, &owner.host)? {
            Probe::Free => {}
            Probe::Live(holder) => return Ok(Err(ReadOnlyCause::Locked(holder))),
            Probe::Stale(holder) => return Ok(Err(ReadOnlyCause::StaleLock(holder))),
        }
    }
    Ok(Err(ReadOnlyCause::Locked(None)))
}

fn locked(holder: Option<&LockHolder>) -> Error {
    Error::new(codes::PACKAGE_LOCKED)
        .with_arg("user", holder.map(|h| h.user.clone()).unwrap_or_default())
        .with_arg("host", holder.map(|h| h.host.clone()).unwrap_or_default())
}

/// Replaces the stale lock of `expected` by a lock of `owner`, when the lock file still names
/// `expected` and is still stale.
pub(crate) fn take_over(package: &Path, expected: &LockHolder, owner: &LockOwner) -> Result<Held> {
    let path = lock_path(package);
    match probe(&path, &owner.host)? {
        Probe::Stale(holder) if &holder == expected => match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error, &path)),
        },
        Probe::Free => {}
        Probe::Stale(holder) => return Err(locked(Some(&holder))),
        Probe::Live(holder) => return Err(locked(holder.as_ref())),
    }
    // Another opener may have taken the lock since the removal.
    create(&path, owner)?.ok_or_else(|| locked(None))
}
