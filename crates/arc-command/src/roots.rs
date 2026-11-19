//! Read and write roots (CORE-CMD-032).
//!
//! Every file an automation surface touches is opened through [`Roots`]: the read roots and
//! write roots granted to a session, canonicalized when granted. Write roots are also read
//! roots. A path is accepted when it is absolute, passes the textual checks below, and its
//! canonical form (`.`, `..`, symbolic links and junctions resolved by the operating system)
//! lies inside a root of the required kind, compared component by component as the operating
//! system returns it. No Unicode normalization or case folding is applied: a variant names
//! whatever object the file system resolves it to, and that object must lie inside a root.
//!
//! The check is bound to the opened handle; data is read or written only through a handle that
//! passed it. What each platform guarantees:
//!
//! - **Unix (Linux, Android, macOS, the BSD systems)**: the root directory is opened, then each
//!   remaining component of the resolved path is opened relative to the previous directory
//!   handle without following a link (`openat` with `O_NOFOLLOW | O_DIRECTORY`), and the file is
//!   opened or created (exclusively) relative to the last one, again without following a link.
//!   A component that another process swaps for a link after the resolution is refused, so
//!   neither a read nor a new file can be redirected outside the roots.
//! - **Windows**: every directory of the resolved path from the root down to the parent is
//!   opened without following reparse points and without sharing delete access, so none of them
//!   can be renamed, deleted or replaced by a junction while the file is opened. A symbolic link
//!   or junction on that chain, or as the file, is refused with `cmd.path.rejected` and reason
//!   `link`. A directory that is another reparse point, such as a cloud-synchronized folder, is
//!   accepted. A file with the reparse-point attribute that is not a link is refused with reason
//!   `reparse_file` and never read, because a file opened without following its reparse point can
//!   be a stub. A file the system presents as an ordinary file is read. Device names, alternate
//!   data streams, and device-namespace paths are rejected; UNC paths only when a root is a UNC
//!   path.
//! - **WebAssembly**: there is no file system; granting a root fails and every access is
//!   `cmd.path.outside_roots`.
//!
//! Everywhere: an existing file is opened for writing without truncation, checked, and only then
//! truncated; a path that names something other than a regular file is rejected; an object
//! opened outside the roots during a race is closed without reading or writing it. Opening a
//! named pipe that an attacker places at a checked path can block.

use std::fs::File;
use std::path::{Path, PathBuf};

use arc_base::{Error, Result};

use crate::error::codes;

/// The longest path accepted, in bytes (UTF-16 code units on Windows).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub const MAX_PATH_LEN: usize = 4095;
/// The longest path accepted, in bytes (UTF-16 code units on Windows).
#[cfg(windows)]
pub const MAX_PATH_LEN: usize = 32_767;
/// The longest path accepted, in bytes (UTF-16 code units on Windows).
#[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
pub const MAX_PATH_LEN: usize = 1023;

/// The canonicalized read and write roots of a session. The default grants nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
}

fn rejected(reason: &'static str) -> Error {
    Error::new(codes::PATH_REJECTED).with_detail("reason", reason)
}

fn outside() -> Error {
    Error::new(codes::PATH_OUTSIDE_ROOTS)
}

impl Roots {
    /// No roots: every access is `cmd.path.outside_roots`.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Grants `read` and `write` roots, each canonicalized. Fails with `cmd.roots.invalid` when
    /// a root is not an existing directory (detail `index`, the position among the read roots
    /// followed by the write roots); on WebAssembly, for any root.
    pub fn new<P: AsRef<Path>>(
        read: impl IntoIterator<Item = P>,
        write: impl IntoIterator<Item = P>,
    ) -> Result<Self> {
        let mut index = 0_i64;
        let mut canonical = |paths: Vec<P>| -> Result<Vec<PathBuf>> {
            let mut out = Vec::new();
            for path in paths {
                let root = native::canonical_dir(path.as_ref())
                    .ok_or_else(|| Error::new(codes::ROOTS_INVALID).with_detail("index", index))?;
                index = index.saturating_add(1);
                if !out.contains(&root) {
                    out.push(root);
                }
            }
            Ok(out)
        };
        let read = canonical(read.into_iter().collect())?;
        let write = canonical(write.into_iter().collect())?;
        Ok(Self { read, write })
    }

    /// The read roots, without the write roots.
    #[must_use]
    pub fn read_roots(&self) -> &[PathBuf] {
        &self.read
    }

    /// The write roots.
    #[must_use]
    pub fn write_roots(&self) -> &[PathBuf] {
        &self.write
    }

    /// The root of the required kind that holds the canonical `path`.
    fn holder(&self, path: &Path, write: bool) -> Option<&Path> {
        let readable = if write { &[][..] } else { &self.read[..] };
        self.write
            .iter()
            .chain(readable)
            .find(|root| path.starts_with(root))
            .map(PathBuf::as_path)
    }

    /// Opens the regular file at `path` for reading.
    ///
    /// # Errors
    ///
    /// `cmd.path.rejected`, `cmd.path.too_long`, `cmd.path.outside_roots`, `cmd.path.not_found`
    /// or `cmd.path.io`.
    pub fn open_read(&self, path: &Path) -> Result<File> {
        check_text(path, self)?;
        native::open_read(self, path)
    }

    /// Opens the file at `path` for writing, creating it when it does not exist and truncating
    /// it when it does. Its parent directory must exist.
    ///
    /// # Errors
    ///
    /// `cmd.path.rejected`, `cmd.path.too_long`, `cmd.path.outside_roots`, `cmd.path.not_found`
    /// or `cmd.path.io`.
    pub fn create_write(&self, path: &Path) -> Result<File> {
        check_text(path, self)?;
        let name = match path.components().next_back() {
            Some(std::path::Component::Normal(name)) => name,
            _ => return Err(rejected("not_a_file_name")),
        };
        let parent = path.parent().ok_or_else(|| rejected("not_a_file_name"))?;
        native::create_write(self, parent, Path::new(name))
    }

    /// Reserves `path` as a new file inside a write root: the file is created exclusively, so
    /// nothing, a link included, may exist under the name, and no existing file is opened or
    /// truncated. The parent directory is resolved and must lie in a write root; a path under a
    /// directory that does not exist is `cmd.path.outside_roots` unless the deepest existing
    /// ancestor lies inside a root. The file stays as an empty placeholder: the caller fills
    /// the path. Returns the resolved path.
    ///
    /// # Errors
    ///
    /// `cmd.path.rejected`, `cmd.path.too_long`, `cmd.path.outside_roots`, `cmd.path.not_found`
    /// or `cmd.path.io` (reason `exists` when the name is taken, `denied` or `other`).
    pub fn reserve(&self, path: &Path) -> Result<PathBuf> {
        check_text(path, self)?;
        let name = match path.components().next_back() {
            Some(std::path::Component::Normal(name)) => name,
            _ => return Err(rejected("not_a_file_name")),
        };
        let parent = path.parent().ok_or_else(|| rejected("not_a_file_name"))?;
        native::reserve(self, parent, Path::new(name))
    }

    /// Checks that [`Roots::create_write`] would accept `path` for a macro or another file the
    /// runner writes at the end: a new name, or an existing regular file, inside a write root.
    /// Nothing is created or truncated.
    ///
    /// # Errors
    ///
    /// As [`Roots::create_write`], without the truncation.
    pub fn check_write(&self, path: &Path) -> Result<()> {
        check_text(path, self)?;
        let parent = path.parent().ok_or_else(|| rejected("not_a_file_name"))?;
        let parent = canonical(self, parent, true)?;
        if self.holder(&parent, true).is_none() {
            return Err(outside());
        }
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                let target = canonical(self, path, true)?;
                self.holder(&target, true).ok_or_else(outside)?;
                self.open_read(path).map(drop)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&error)),
        }
    }
}

fn exists_error() -> Error {
    Error::new(codes::PATH_IO).with_detail("reason", "exists")
}

/// The checks of a path before the file system sees it.
fn check_text(path: &Path, roots: &Roots) -> Result<()> {
    let text = path.as_os_str();
    if text.is_empty() {
        return Err(rejected("empty"));
    }
    if path_len(path) > MAX_PATH_LEN {
        return Err(Error::new(codes::PATH_TOO_LONG));
    }
    if text.to_string_lossy().contains('\0') {
        return Err(rejected("nul"));
    }
    if !path.is_absolute() {
        return Err(rejected("relative"));
    }
    #[cfg(windows)]
    windows_checks(path, roots)?;
    let _ = roots;
    Ok(())
}

#[cfg(windows)]
fn path_len(path: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count()
}

#[cfg(not(windows))]
fn path_len(path: &Path) -> usize {
    path.as_os_str().len()
}

#[cfg(windows)]
fn windows_checks(path: &Path, roots: &Roots) -> Result<()> {
    use std::path::{Component, Prefix};
    let unc_root =
        roots
            .read
            .iter()
            .chain(&roots.write)
            .any(|root| match root.components().next() {
                Some(Component::Prefix(p)) => {
                    matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
                }
                _ => false,
            });
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) if unc_root => {}
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) => return Err(rejected("network")),
                Prefix::DeviceNS(_) | Prefix::Verbatim(_) => return Err(rejected("device")),
            },
            Component::Normal(name) => {
                if let Some(reason) = windows_name_problem(&name.to_string_lossy()) {
                    return Err(rejected(reason));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Why a path component is unacceptable on Windows: an alternate data stream (`:`) or a
/// reserved device name, with any extension, ignoring case and trailing dots and spaces.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used on Windows, tested everywhere")
)]
pub(crate) fn windows_name_problem(name: &str) -> Option<&'static str> {
    if name.contains(':') {
        return Some("alternate_data_stream");
    }
    let stem = name.split('.').next().unwrap_or_default();
    let stem = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|digit| {
            matches!(
                digit,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    device.then_some("device_name")
}

/// Maps an operating-system error without its text, which can name host paths.
#[cfg_attr(not(any(unix, windows)), allow(dead_code, reason = "no file system"))]
fn io_error(error: &std::io::Error) -> Error {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => Error::new(codes::PATH_NOT_FOUND),
        ErrorKind::InvalidFilename => Error::new(codes::PATH_TOO_LONG),
        ErrorKind::PermissionDenied => Error::new(codes::PATH_IO).with_detail("reason", "denied"),
        ErrorKind::AlreadyExists => Error::new(codes::PATH_IO).with_detail("reason", "exists"),
        ErrorKind::NotADirectory => Error::new(codes::PATH_NOT_FOUND),
        _ => Error::new(codes::PATH_IO).with_detail("reason", "other"),
    }
}

/// Canonicalizes `path`. When that fails, the error is `cmd.path.outside_roots` unless the
/// deepest ancestor that resolves lies inside a root of the required kind, so errors reveal
/// nothing about what exists outside the roots.
#[cfg_attr(not(any(unix, windows)), allow(dead_code, reason = "no file system"))]
fn canonical(roots: &Roots, path: &Path, write: bool) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|e| {
        let inside = path
            .ancestors()
            .skip(1)
            .find_map(|a| std::fs::canonicalize(a).ok())
            .is_some_and(|a| roots.holder(&a, write).is_some());
        if inside { io_error(&e) } else { outside() }
    })
}

#[cfg(not(any(unix, windows)))]
mod native {
    use std::fs::File;
    use std::path::{Path, PathBuf};

    use arc_base::Result;

    use super::{Roots, outside};

    pub(super) fn canonical_dir(_: &Path) -> Option<PathBuf> {
        None
    }

    pub(super) fn open_read(_: &Roots, _: &Path) -> Result<File> {
        Err(outside())
    }

    pub(super) fn create_write(_: &Roots, _: &Path, _: &Path) -> Result<File> {
        Err(outside())
    }

    pub(super) fn reserve(_: &Roots, _: &Path, _: &Path) -> Result<PathBuf> {
        Err(outside())
    }
}

#[cfg(unix)]
mod native {
    use std::fs::{self, File};
    use std::os::fd::{AsFd, OwnedFd};
    use std::path::{Component, Path, PathBuf};

    use arc_base::{Error, Result};
    use rustix::fs::{CWD, Mode, OFlags, openat};
    use rustix::io::Errno;

    use super::{Roots, canonical, exists_error, io_error, outside, rejected};

    pub(super) fn canonical_dir(path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok().filter(|p| p.is_dir())
    }

    /// A link met where the resolved path had none (`ELOOP`, `EMLINK` on FreeBSD) or a
    /// directory swapped for something else means the tree changed after resolution.
    fn open_error(e: Errno) -> Error {
        if [Errno::LOOP, Errno::MLINK, Errno::NOTDIR].contains(&e) {
            outside()
        } else {
            io_error(&e.into())
        }
    }

    /// Opens `name` below `dir` without following a link in it.
    fn at(dir: impl AsFd, name: &Path, flags: OFlags) -> Result<OwnedFd> {
        let flags = flags | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        openat(dir, name, flags, Mode::from_bits_truncate(0o666)).map_err(open_error)
    }

    /// Opens the canonical directory `dir` below `root` one component at a time, never through
    /// a link, so a component swapped for a link after resolution is refused, not followed.
    fn walk(root: &Path, dir: &Path) -> Result<OwnedFd> {
        let rest = dir.strip_prefix(root).map_err(|_| outside())?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY;
        let mut handle =
            openat(CWD, root, flags | OFlags::CLOEXEC, Mode::empty()).map_err(open_error)?;
        for component in rest.components() {
            let Component::Normal(name) = component else {
                return Err(outside());
            };
            handle = at(&handle, Path::new(name), flags)?;
        }
        Ok(handle)
    }

    /// Opens the existing regular file at the canonical path `path` through its directory chain.
    fn open_canonical(roots: &Roots, path: &Path, write: bool) -> Result<File> {
        let root = roots.holder(path, write).ok_or_else(outside)?;
        if !fs::metadata(path).is_ok_and(|m| m.is_file()) {
            return Err(rejected("not_a_file"));
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(rejected("not_a_file"));
        };
        let dir = walk(root, parent)?;
        let mode = if write {
            OFlags::WRONLY
        } else {
            OFlags::RDONLY
        };
        let file = File::from(at(&dir, Path::new(name), mode)?);
        if !file.metadata().map_err(|e| io_error(&e))?.is_file() {
            return Err(rejected("not_a_file"));
        }
        Ok(file)
    }

    /// Creates `name` in the canonical directory `parent`, or opens and then truncates the
    /// regular file already there.
    /// Creates `name` in the directory `parent`, which must lie in a write root. `exclusive` makes
    /// an existing name an error; otherwise the existing file is opened and truncated.
    fn create_canonical(
        roots: &Roots,
        parent: &Path,
        name: &Path,
        exclusive: bool,
    ) -> Result<File> {
        let root = roots.holder(parent, true).ok_or_else(outside)?;
        let dir = walk(root, parent)?;
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL;
        match openat(
            &dir,
            name,
            flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o666),
        ) {
            Ok(fd) => Ok(File::from(fd)),
            // An existing name, a link among them, is resolved and checked like a read.
            Err(e) if e == Errno::EXIST && exclusive => Err(exists_error()),
            Err(e) if e == Errno::EXIST => {
                drop(dir);
                let target = canonical(roots, &parent.join(name), true)?;
                let file = open_canonical(roots, &target, true)?;
                file.set_len(0).map_err(|e| io_error(&e))?;
                Ok(file)
            }
            Err(e) => Err(open_error(e)),
        }
    }

    pub(super) fn open_read(roots: &Roots, path: &Path) -> Result<File> {
        open_canonical(roots, &canonical(roots, path, false)?, false)
    }

    pub(super) fn create_write(roots: &Roots, parent: &Path, name: &Path) -> Result<File> {
        create_canonical(roots, &canonical(roots, parent, true)?, name, false)
    }

    pub(super) fn reserve(roots: &Roots, parent: &Path, name: &Path) -> Result<PathBuf> {
        let parent = canonical(roots, parent, true)?;
        drop(create_canonical(roots, &parent, name, true)?);
        Ok(parent.join(name))
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used, reason = "tests")]

        use super::*;

        /// A directory inside the root that becomes a link to outside after its path was
        /// resolved is refused for reading and creating, and nothing appears outside.
        #[test]
        fn a_directory_replaced_by_a_link_after_resolution_is_refused() {
            let base = std::env::temp_dir().join(format!("arc-roots-swap-{}", std::process::id()));
            let (root, outside_dir) = (base.join("root"), base.join("outside"));
            fs::create_dir_all(root.join("d")).unwrap();
            fs::create_dir_all(&outside_dir).unwrap();
            fs::write(outside_dir.join("canary.txt"), "CANARY").unwrap();
            let roots = Roots::new([&root], [&root]).unwrap();
            let dir = fs::canonicalize(root.join("d")).unwrap();
            // The swap the resolution did not see.
            fs::remove_dir(&dir).unwrap();
            std::os::unix::fs::symlink(&outside_dir, &dir).unwrap();
            let code = |r: Result<File>| r.map(drop).unwrap_err().code().to_owned();
            assert_eq!(
                code(open_canonical(&roots, &dir.join("canary.txt"), false)),
                "cmd.path.outside_roots"
            );
            assert_eq!(
                code(open_canonical(&roots, &dir.join("canary.txt"), true)),
                "cmd.path.outside_roots"
            );
            assert_eq!(
                code(create_canonical(&roots, &dir, Path::new("new.txt"), false)),
                "cmd.path.outside_roots"
            );
            let entries: Vec<_> = fs::read_dir(&outside_dir).unwrap().collect();
            let canary = fs::read_to_string(outside_dir.join("canary.txt"));
            let _ = fs::remove_dir_all(&base);
            assert_eq!(entries.len(), 1, "a file appeared outside the roots");
            assert_eq!(canary.unwrap(), "CANARY");
        }
    }
}

#[cfg(windows)]
mod native {
    use std::fs::{self, File, OpenOptions};
    use std::io::ErrorKind;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    use arc_base::Result;

    use super::{Roots, canonical, exists_error, io_error, outside, rejected};

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    pub(super) fn canonical_dir(path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok().filter(|p| p.is_dir())
    }

    /// Checks the handle is no link and has the expected kind. `is_symlink` on Windows is true
    /// for name-surrogate reparse points (symbolic links and junctions). A directory that is
    /// another reparse point is accepted; a file with the reparse-point attribute is refused, as
    /// its data may not be present locally.
    fn plain(file: &File, dir: bool) -> Result<()> {
        let meta = file.metadata().map_err(|e| io_error(&e))?;
        if meta.file_type().is_symlink() {
            return Err(rejected("link"));
        }
        if !dir && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(rejected("reparse_file"));
        }
        if dir != meta.is_dir() || (!dir && !meta.is_file()) {
            return Err(rejected(if dir { "not_a_directory" } else { "not_a_file" }));
        }
        Ok(())
    }

    /// Opens every directory from `root` down to `dir` without delete sharing, so the chain
    /// cannot change while the handles are held.
    fn hold_chain(root: &Path, dir: &Path) -> Result<Vec<File>> {
        let rest = dir.strip_prefix(root).map_err(|_| outside())?;
        let mut held = Vec::new();
        let mut at = root.to_path_buf();
        let mut open = |at: &Path| -> Result<()> {
            let handle = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(at)
                .map_err(|e| io_error(&e))?;
            plain(&handle, true)?;
            held.push(handle);
            Ok(())
        };
        open(&at)?;
        for component in rest.components() {
            at.push(component);
            open(&at)?;
        }
        Ok(held)
    }

    fn split(roots: &Roots, canonical: &Path, write: bool) -> Result<(PathBuf, PathBuf)> {
        let root = roots.holder(canonical, write).ok_or_else(outside)?;
        let parent = canonical.parent().ok_or_else(|| rejected("not_a_file"))?;
        Ok((root.to_path_buf(), parent.to_path_buf()))
    }

    /// Opens the existing regular file at `path` with `options` while its canonical directory
    /// chain is held.
    fn open_existing(
        roots: &Roots,
        path: &Path,
        write: bool,
        options: &OpenOptions,
    ) -> Result<File> {
        let canonical = canonical(roots, path, write)?;
        let (root, parent) = split(roots, &canonical, write)?;
        if !fs::metadata(&canonical).is_ok_and(|m| m.is_file()) {
            return Err(rejected("not_a_file"));
        }
        let held = hold_chain(&root, &parent)?;
        let file = options.open(&canonical).map_err(|e| io_error(&e))?;
        plain(&file, false)?;
        drop(held);
        Ok(file)
    }

    fn options(write: bool, create: bool) -> OpenOptions {
        let mut options = OpenOptions::new();
        options
            .read(!write)
            .write(write)
            .create_new(create)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        options
    }

    pub(super) fn open_read(roots: &Roots, path: &Path) -> Result<File> {
        open_existing(roots, path, false, &options(false, false))
    }

    /// Creates the file exclusively under the held directory chain; nothing may exist under the
    /// name, a link included.
    pub(super) fn reserve(roots: &Roots, parent: &Path, name: &Path) -> Result<PathBuf> {
        let parent = canonical(roots, parent, true)?;
        let root = roots
            .holder(&parent, true)
            .ok_or_else(outside)?
            .to_path_buf();
        let held = hold_chain(&root, &parent)?;
        let target = parent.join(name);
        let created = options(true, true).open(&target);
        drop(held);
        let file = created.map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => exists_error(),
            _ => io_error(&e),
        })?;
        plain(&file, false)?;
        Ok(target)
    }

    pub(super) fn create_write(roots: &Roots, parent: &Path, name: &Path) -> Result<File> {
        let parent = canonical(roots, parent, true)?;
        let root = roots
            .holder(&parent, true)
            .ok_or_else(outside)?
            .to_path_buf();
        let held = hold_chain(&root, &parent)?;
        let target = parent.join(name);
        // A directory of that name is refused as a directory, as on Unix. Without this check the
        // open below fails inside the operating system with a generic error. The link itself is
        // inspected, not followed: a link is resolved below like on Unix.
        if fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir()) {
            return Err(rejected("not_a_file"));
        }
        let created = options(true, true).open(&target);
        drop(held);
        let file = match created {
            Ok(file) => {
                plain(&file, false)?;
                return Ok(file);
            }
            // An existing name, a link among them, is resolved and checked like a read.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                open_existing(roots, &target, true, &options(true, false))?
            }
            Err(e) => return Err(io_error(&e)),
        };
        file.set_len(0).map_err(|e| io_error(&e))?;
        Ok(file)
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used, reason = "tests")]

        use super::*;

        /// A junction is refused as a link with a reason, while a plain directory passes the same
        /// handle check. Non-link reparse points (cloud placeholders) cannot be created in a test.
        #[test]
        fn a_junction_is_refused_as_a_link_and_a_directory_is_not() {
            let base = std::env::temp_dir().join(format!("arc-roots-jn-{}", std::process::id()));
            let (target, link) = (base.join("t"), base.join("j"));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&target).unwrap();
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .is_ok_and(|o| o.status.success());
            if !made {
                let _ = fs::remove_dir_all(&base);
                return;
            }
            let open = |p: &Path| {
                OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(p)
                    .unwrap()
            };
            let refused = {
                let handle = open(&link);
                plain(&handle, true).unwrap_err()
            };
            let accepted = plain(&open(&target), true);
            let reason = refused.details().get("reason").cloned();
            let _ = fs::remove_dir(&link);
            let _ = fs::remove_dir_all(&base);
            assert_eq!(refused.code(), "cmd.path.rejected");
            assert_eq!(
                reason,
                Some(arc_base::error::Value::Text("link".to_owned()))
            );
            assert!(accepted.is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_names_are_checked_on_every_platform() {
        for bad in [
            "CON",
            "con.txt",
            "Nul ",
            "aux.tar.gz",
            "COM1",
            "lpt9.log",
            "CONIN$",
        ] {
            assert_eq!(windows_name_problem(bad), Some("device_name"), "{bad}");
        }
        assert_eq!(
            windows_name_problem("file.txt:stream"),
            Some("alternate_data_stream")
        );
        for good in ["console", "COM10", "nul_file", "a.con", "LPT"] {
            assert_eq!(windows_name_problem(good), None, "{good}");
        }
    }

    #[test]
    fn text_checks_reject_before_the_file_system() {
        let roots = Roots::none();
        let code = |p: &str| roots.open_read(Path::new(p)).map(drop).unwrap_err().code();
        assert_eq!(code(""), "cmd.path.rejected");
        assert_eq!(code("relative/file"), "cmd.path.rejected");
        assert_eq!(code("/a\0b"), "cmd.path.rejected");
        let long = format!("/{}", "a".repeat(MAX_PATH_LEN));
        assert_eq!(code(&long), "cmd.path.too_long");
    }

    #[test]
    fn no_roots_deny_everything() {
        let roots = Roots::none();
        let here = std::env::temp_dir();
        let err = roots.create_write(&here.join("x")).unwrap_err();
        assert_eq!(err.code(), "cmd.path.outside_roots");
    }
}
