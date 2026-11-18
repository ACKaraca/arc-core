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
//! - **Linux and Android**: the location of the opened handle is read back from
//!   `/proc/self/fd` and must lie inside a root. A new file is created through the verified
//!   handle of its parent directory (`/proc/self/fd/<dir>/<name>` with exclusive creation), so a
//!   directory swapped for a link after the check cannot redirect the creation. Without `/proc`
//!   the generic Unix check below applies.
//! - **Other Unix systems (macOS)**: after opening, the device and inode of the handle must equal
//!   those of the path resolved again, which must still lie inside a root. A swap between the
//!   check and the open is detected and the handle refused before any data is read or written.
//!   Creating a new file while another process swaps a directory inside the root for a link can
//!   leave an empty file outside the roots; it is detected, and its handle refused.
//! - **Windows**: every directory of the resolved path from the root down to the parent is
//!   opened without following reparse points and without sharing delete access, so none of them
//!   can be renamed, deleted or replaced by a junction while the file is opened; a reparse point
//!   on that chain is refused. The file itself is opened without following a reparse point. Device names,
//!   alternate data streams, and device-namespace paths are rejected; UNC paths only when a root
//!   is a UNC path.
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
#[cfg_attr(target_arch = "wasm32", allow(dead_code, reason = "no file system"))]
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
#[cfg_attr(target_arch = "wasm32", allow(dead_code, reason = "no file system"))]
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

#[cfg(target_arch = "wasm32")]
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
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
mod native {
    use std::fs::{self, File, OpenOptions};
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    use arc_base::Result;

    use super::{Roots, canonical, io_error, outside, rejected};

    pub(super) fn canonical_dir(path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok().filter(|p| p.is_dir())
    }

    /// Where the handle is, read back from `/proc/self/fd`.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn handle_path(file: &File) -> Option<PathBuf> {
        use std::os::fd::AsRawFd;
        fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn handle_path(_: &File) -> Option<PathBuf> {
        None
    }

    /// `O_NOFOLLOW` where the handle's location cannot be read back: the open fails when the
    /// last component is a link, so a file swapped for a link is never opened through it.
    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    const NO_FOLLOW: i32 = 0x0100;

    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    )))]
    const NO_FOLLOW: i32 = 0;

    /// Open options for a file below a verified directory.
    fn options() -> OpenOptions {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.custom_flags(NO_FOLLOW);
        options
    }

    /// Verifies that the opened `file`, reached through the canonical path `expected`, lies
    /// inside a root of the required kind and is a regular file (a directory when `dir`).
    fn verify(roots: &Roots, file: &File, expected: &Path, write: bool, dir: bool) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().map_err(|e| io_error(&e))?;
        if dir != meta.is_dir() || (!dir && !meta.is_file()) {
            return Err(rejected(if dir { "not_a_directory" } else { "not_a_file" }));
        }
        if let Some(real) = handle_path(file) {
            return roots.holder(&real, write).map(drop).ok_or_else(outside);
        }
        let now = fs::canonicalize(expected).map_err(|_| outside())?;
        // Not following a link: the path may have been swapped for one since it was resolved.
        let again = fs::symlink_metadata(&now).map_err(|_| outside())?;
        let same = again.dev() == meta.dev() && again.ino() == meta.ino();
        if same && roots.holder(&now, write).is_some() {
            Ok(())
        } else {
            Err(outside())
        }
    }

    pub(super) fn open_read(roots: &Roots, path: &Path) -> Result<File> {
        let canonical = canonical(roots, path, false)?;
        roots.holder(&canonical, false).ok_or_else(outside)?;
        if !fs::metadata(&canonical).is_ok_and(|m| m.is_file()) {
            return Err(rejected("not_a_file"));
        }
        let file = options()
            .read(true)
            .open(&canonical)
            .map_err(|e| io_error(&e))?;
        verify(roots, &file, &canonical, false, false)?;
        Ok(file)
    }

    pub(super) fn create_write(roots: &Roots, parent: &Path, name: &Path) -> Result<File> {
        let parent = canonical(roots, parent, true)?;
        roots.holder(&parent, true).ok_or_else(outside)?;
        let target = parent.join(name);
        // On Linux the file is reached through the verified handle of its directory.
        let dir = File::open(&parent).map_err(|e| io_error(&e))?;
        verify(roots, &dir, &parent, true, true)?;
        let via = handle_path(&dir).map_or_else(
            || target.clone(),
            |_| {
                use std::os::fd::AsRawFd;
                PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name)
            },
        );
        let created = options().write(true).create_new(true).open(&via);
        let (file, existed) = match created {
            Ok(file) => (file, false),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let canonical = canonical(roots, &target, true)?;
                roots.holder(&canonical, true).ok_or_else(outside)?;
                if !fs::metadata(&canonical).is_ok_and(|m| m.is_file()) {
                    return Err(rejected("not_a_file"));
                }
                let file = options().write(true).open(&via).map_err(|e| io_error(&e))?;
                (file, true)
            }
            Err(e) => return Err(io_error(&e)),
        };
        verify(roots, &file, &target, true, false)?;
        drop(dir);
        if existed {
            file.set_len(0).map_err(|e| io_error(&e))?;
        }
        Ok(file)
    }
}

#[cfg(windows)]
mod native {
    use std::fs::{self, File, OpenOptions};
    use std::io::ErrorKind;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    use arc_base::Result;

    use super::{Roots, canonical, io_error, outside, rejected};

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    pub(super) fn canonical_dir(path: &Path) -> Option<PathBuf> {
        fs::canonicalize(path).ok().filter(|p| p.is_dir())
    }

    /// Checks the handle is no reparse point and has the expected kind.
    fn plain(file: &File, dir: bool) -> Result<()> {
        let meta = file.metadata().map_err(|e| io_error(&e))?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(outside());
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

    pub(super) fn create_write(roots: &Roots, parent: &Path, name: &Path) -> Result<File> {
        let parent = canonical(roots, parent, true)?;
        let root = roots
            .holder(&parent, true)
            .ok_or_else(outside)?
            .to_path_buf();
        let held = hold_chain(&root, &parent)?;
        let target = parent.join(name);
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
