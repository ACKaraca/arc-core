//! The recent-files model (CORE-APP-007): a bounded, most-recent-first list with pinned entries,
//! and the thumbnails stored beside it ([`Thumbnails`]).
//!
//! Paths are compared as given; the caller normalizes them. The list is persisted as its own TOML
//! file through [`settings_file`](crate::settings_file), so a malformed file is set aside the same
//! way. Entries are never removed because a file is missing unless [`RecentFiles::prune_missing`]
//! is called.

use std::io::Read;
use std::path::{Path, PathBuf};

use arc_base::{Error, Result};
use toml::{Table, Value};

use crate::codes;
use crate::settings_file::{self, SetAside};

/// The most pinned entries a list holds.
pub const MAX_PINNED: usize = 50;
/// The largest accepted bound on unpinned entries.
pub const MAX_ENTRIES: usize = 1000;

/// One entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentEntry {
    /// The file path.
    pub path: PathBuf,
    /// Whether the entry is pinned.
    pub pinned: bool,
}

/// The recent-files list. Pinned entries are never evicted; `max_entries` bounds the unpinned
/// ones, and the oldest unpinned entry is dropped first.
#[derive(Debug, Clone)]
pub struct RecentFiles {
    // Most recently used first, pinned and unpinned interleaved.
    entries: Vec<RecentEntry>,
    max_entries: usize,
}

impl RecentFiles {
    /// An empty list; `max_entries` is clamped to `1..=`[`MAX_ENTRIES`].
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.clamp(1, MAX_ENTRIES),
        }
    }

    /// The entries to show: pinned ones first, then the others, each most recent first.
    pub fn iter(&self) -> impl Iterator<Item = &RecentEntry> {
        let pinned = self.entries.iter().filter(|e| e.pinned);
        pinned.chain(self.entries.iter().filter(|e| !e.pinned))
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn enforce_bound(&mut self) {
        let mut unpinned = self.entries.iter().filter(|e| !e.pinned).count();
        while unpinned > self.max_entries {
            if let Some(at) = self.entries.iter().rposition(|e| !e.pinned) {
                self.entries.remove(at);
            }
            unpinned -= 1;
        }
    }

    /// Records that a file was opened or saved: it moves to the front, keeping its pin.
    pub fn touch(&mut self, path: &Path) {
        let pinned = self
            .entries
            .iter()
            .position(|e| e.path == path)
            .is_some_and(|at| self.entries.remove(at).pinned);
        self.entries.insert(
            0,
            RecentEntry {
                path: path.to_owned(),
                pinned,
            },
        );
        self.enforce_bound();
    }

    fn position(&self, path: &Path) -> Result<usize> {
        self.entries
            .iter()
            .position(|e| e.path == path)
            .ok_or_else(|| {
                Error::new(codes::RECENT_NOT_FOUND).with_arg("path", path.display().to_string())
            })
    }

    /// Pins an entry.
    ///
    /// # Errors
    /// `app.recent.not_found` or `app.recent.too_many_pinned`.
    pub fn pin(&mut self, path: &Path) -> Result<()> {
        let at = self.position(path)?;
        if !self.entries[at].pinned
            && self.entries.iter().filter(|e| e.pinned).count() >= MAX_PINNED
        {
            return Err(Error::new(codes::RECENT_TOO_MANY_PINNED)
                .with_detail("limit", i64::try_from(MAX_PINNED).unwrap_or(i64::MAX)));
        }
        self.entries[at].pinned = true;
        Ok(())
    }

    /// Unpins an entry; it keeps its recency position and may be evicted by the bound.
    ///
    /// # Errors
    /// `app.recent.not_found`.
    pub fn unpin(&mut self, path: &Path) -> Result<()> {
        let at = self.position(path)?;
        self.entries[at].pinned = false;
        self.enforce_bound();
        Ok(())
    }

    /// Removes one entry, pinned or not.
    ///
    /// # Errors
    /// `app.recent.not_found`.
    pub fn remove(&mut self, path: &Path) -> Result<()> {
        let at = self.position(path)?;
        self.entries.remove(at);
        Ok(())
    }

    /// Removes every entry for which `exists` is false, pinned ones included, and returns the
    /// removed paths. Call it only on an explicit user request.
    pub fn prune_missing(&mut self, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let (keep, gone): (Vec<_>, Vec<_>) = self.entries.drain(..).partition(|e| exists(&e.path));
        self.entries = keep;
        gone.into_iter().map(|e| e.path).collect()
    }

    /// The list as a TOML table. Entries whose path is not valid UTF-8 are not persisted.
    #[must_use]
    pub fn to_table(&self) -> Table {
        let items = self
            .entries
            .iter()
            .filter_map(|e| {
                let mut t = Table::new();
                t.insert("path".into(), Value::String(e.path.to_str()?.to_owned()));
                t.insert("pinned".into(), Value::Boolean(e.pinned));
                Some(Value::Table(t))
            })
            .collect();
        let mut table = Table::new();
        table.insert("entries".into(), Value::Array(items));
        table
    }

    /// Reads a list from a table. Malformed items and duplicates are skipped, surplus pins are
    /// unpinned and the bound is applied.
    #[must_use]
    pub fn from_table(table: &Table, max_entries: usize) -> Self {
        let mut list = Self::new(max_entries);
        let items = table
            .get("entries")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mut pinned = 0;
        for item in items.iter().take(2 * MAX_ENTRIES) {
            let Some(path) = item.get("path").and_then(Value::as_str) else {
                continue;
            };
            if list.entries.iter().any(|e| e.path.as_os_str() == path) {
                continue;
            }
            let wants = item.get("pinned").and_then(Value::as_bool).unwrap_or(false);
            let is_pinned = wants && pinned < MAX_PINNED;
            pinned += usize::from(is_pinned);
            list.entries.push(RecentEntry {
                path: PathBuf::from(path),
                pinned: is_pinned,
            });
        }
        list.enforce_bound();
        list
    }

    /// Loads the list from `path`; a malformed file is set aside as for settings files.
    ///
    /// # Errors
    /// As [`settings_file::load`].
    pub fn load(path: &Path, max_entries: usize) -> Result<(Self, Option<SetAside>)> {
        let loaded = settings_file::load(path)?;
        Ok((
            Self::from_table(&loaded.table, max_entries),
            loaded.set_aside,
        ))
    }

    /// Saves the list atomically.
    ///
    /// # Errors
    /// As [`settings_file::save`].
    pub fn save(&self, path: &Path) -> Result<()> {
        settings_file::save(path, &self.to_table())
    }
}

/// The largest thumbnail file, in bytes.
pub const MAX_THUMBNAIL_BYTES: usize = 64 << 10;
/// The largest thumbnail width or height, in pixels.
pub const MAX_THUMBNAIL_SIDE: u32 = 256;
/// The most files a [`Thumbnails::sweep`] looks at.
const MAX_SWEEP_FILES: usize = 10_000;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The IEND chunk: an empty data field, the type and the CRC that every PNG file ends with.
const IEND_CHUNK: [u8; 12] = [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82];

/// The length of the smallest PNG: the signature, an IHDR chunk and the IEND chunk.
const MIN_PNG_BYTES: usize = 8 + 25 + IEND_CHUNK.len();

/// Thumbnails of recent files: one PNG file per path in a directory the caller chooses, named by
/// a hash of the path. The directory is never inside the settings file, so a thumbnail is never
/// parsed as settings. The crate does not render thumbnails; the caller supplies the PNG bytes.
#[derive(Debug, Clone)]
pub struct Thumbnails {
    dir: PathBuf,
}

/// FNV-1a over the bytes of a path. It is stable across builds, unlike the standard hasher.
fn path_hash(path: &Path) -> u64 {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// The file name of a path's thumbnail.
fn file_name(path: &Path) -> String {
    format!("{:016x}.png", path_hash(path))
}

/// Whether `name` is a thumbnail file name: 16 lowercase hexadecimal digits and `.png`.
fn is_thumbnail_name(name: &str) -> bool {
    name.strip_suffix(".png").is_some_and(|hash| {
        hash.len() == 16
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Whether `name` is a temporary file [`Thumbnails::put`] writes: a thumbnail name with a leading
/// dot and the suffix `.tmp`.
fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(is_thumbnail_name)
}

/// Creates `dir` and any missing parent; on Unix the directories are private to the user.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes `bytes` to `path`; on Unix a created file is readable and writable by the user only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Checks the byte bound, the PNG signature, an IHDR chunk of length 13 with non-zero sides within
/// the pixel bound, and that the file ends with an IEND chunk. Pixels are not decoded.
fn check_png(bytes: &[u8]) -> Result<()> {
    let at = |code| Error::new(code);
    if bytes.len() > MAX_THUMBNAIL_BYTES {
        return Err(at(codes::THUMBNAIL_TOO_LARGE).with_detail(
            "limit",
            i64::try_from(MAX_THUMBNAIL_BYTES).unwrap_or(i64::MAX),
        ));
    }
    if bytes.len() < MIN_PNG_BYTES || !bytes.ends_with(&IEND_CHUNK) {
        return Err(at(codes::THUMBNAIL_INVALID));
    }
    let header = bytes
        .get(..24)
        .ok_or_else(|| at(codes::THUMBNAIL_INVALID))?;
    if header[..8] != PNG_SIGNATURE
        || header[8..12] != 13u32.to_be_bytes()
        || &header[12..16] != b"IHDR"
    {
        return Err(at(codes::THUMBNAIL_INVALID));
    }
    let side = |range: std::ops::Range<usize>| {
        let mut word = [0u8; 4];
        word.copy_from_slice(&header[range]);
        u32::from_be_bytes(word)
    };
    let (width, height) = (side(16..20), side(20..24));
    if width == 0 || height == 0 {
        return Err(at(codes::THUMBNAIL_INVALID));
    }
    if width > MAX_THUMBNAIL_SIDE || height > MAX_THUMBNAIL_SIDE {
        return Err(
            at(codes::THUMBNAIL_TOO_LARGE).with_detail("limit", i64::from(MAX_THUMBNAIL_SIDE))
        );
    }
    Ok(())
}

impl Thumbnails {
    /// The thumbnails kept in `dir`, which is created on the first [`put`](Self::put).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Stores the thumbnail of `path`, replacing an earlier one. The bytes must be a PNG within
    /// [`MAX_THUMBNAIL_BYTES`] and [`MAX_THUMBNAIL_SIDE`] pixels on each side.
    ///
    /// # Errors
    /// `app.thumbnail.too_large` (bytes or pixels above the bounds), `app.thumbnail.invalid`
    /// (not a PNG image header) and `app.thumbnail.io` (the file cannot be written; a failed
    /// write leaves the previous thumbnail in place).
    pub fn put(&self, path: &Path, png: &[u8]) -> Result<()> {
        check_png(png)?;
        let io = |e: std::io::Error| Error::new(codes::THUMBNAIL_IO).with_source(e);
        create_private_dir(&self.dir).map_err(io)?;
        let name = file_name(path);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let target = self.dir.join(&name);
        let written = write_private(&tmp, png).and_then(|()| std::fs::rename(&tmp, &target));
        written.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }

    /// The thumbnail of `path`, or `None` when there is none, the file is missing, too large or
    /// not a valid PNG header. Nothing is reported: a thumbnail is optional.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<Vec<u8>> {
        let target = self.dir.join(file_name(path));
        // The type is checked before opening as well: opening a named pipe waits for a writer.
        if !std::fs::metadata(&target).ok()?.is_file() {
            return None;
        }
        let file = std::fs::File::open(&target).ok()?;
        let limit = u64::try_from(MAX_THUMBNAIL_BYTES).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > limit {
            return None;
        }
        // One byte past the limit is read at most, so a file that grew is refused too.
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() > MAX_THUMBNAIL_BYTES {
            return None;
        }
        check_png(&bytes).ok().map(|()| bytes)
    }

    /// Removes the thumbnail of `path`. A missing thumbnail is not an error.
    ///
    /// # Errors
    /// `app.thumbnail.io` when an existing file cannot be removed.
    pub fn remove(&self, path: &Path) -> Result<()> {
        match std::fs::remove_file(self.dir.join(file_name(path))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::new(codes::THUMBNAIL_IO).with_source(e)),
        }
    }

    /// Removes every thumbnail file whose path is not an entry of `list`, and every temporary file
    /// [`put`](Self::put) left behind. Call it after the list was saved: evicted, removed and
    /// pruned entries then lose their thumbnails too. Other files are left alone; a thumbnail name
    /// is exactly 16 lowercase hexadecimal digits and `.png`.
    ///
    /// # Errors
    /// `app.thumbnail.io` when the directory cannot be read or a stale file cannot be removed.
    pub fn sweep(&self, list: &RecentFiles) -> Result<()> {
        let io = |e: std::io::Error| Error::new(codes::THUMBNAIL_IO).with_source(e);
        let keep: std::collections::BTreeSet<String> =
            list.iter().map(|e| file_name(&e.path)).collect();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
        };
        for entry in entries.take(MAX_SWEEP_FILES) {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale =
                is_temporary_name(&name) || (is_thumbnail_name(&name) && !keep.contains(&name));
            if stale {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    // Removed by another process in the meantime.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io(e)),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;
    use crate::testutil::dir;

    /// A PNG signature, IHDR and IEND chunks of the given size, padded to `len` bytes.
    fn png(width: u32, height: u32, len: usize) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        // Bit depth, colour type, compression, filter and interlace, then a CRC that is not checked.
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes.resize(
            len.max(bytes.len() + IEND_CHUNK.len()) - IEND_CHUNK.len(),
            0,
        );
        bytes.extend_from_slice(&IEND_CHUNK);
        bytes
    }

    #[test]
    fn a_header_without_an_iend_chunk_or_with_another_length_is_not_a_png() {
        let d = dir("thumb-format");
        let t = Thumbnails::new(d.join("thumbs"));
        let p = Path::new("/d/a.arc");
        let mut stub = png(8, 8, 0);
        stub.truncate(24);
        assert_eq!(t.put(p, &stub).unwrap_err().code(), "app.thumbnail.invalid");
        let mut no_end = png(8, 8, 64);
        no_end.truncate(63);
        assert_eq!(
            t.put(p, &no_end).unwrap_err().code(),
            "app.thumbnail.invalid"
        );
        let mut wrong_length = png(8, 8, 64);
        wrong_length[11] = 12;
        assert_eq!(
            t.put(p, &wrong_length).unwrap_err().code(),
            "app.thumbnail.invalid"
        );
    }

    #[cfg(unix)]
    #[test]
    fn thumbnails_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("thumb-private");
        let t = Thumbnails::new(d.join("thumbs"));
        let p = Path::new("/d/a.arc");
        t.put(p, &png(8, 8, 64)).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&d.join("thumbs")), 0o700);
        assert_eq!(mode(&d.join("thumbs").join(file_name(p))), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_thumbnail_that_is_a_device_is_refused_without_being_read() {
        let d = dir("thumb-device");
        let t = Thumbnails::new(d.join("thumbs"));
        let p = Path::new("/d/a.arc");
        std::fs::create_dir_all(d.join("thumbs")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", d.join("thumbs").join(file_name(p))).unwrap();
        assert_eq!(t.get(p), None);
    }

    #[test]
    fn sweep_matches_only_thumbnail_names_and_its_temporary_files() {
        let d = dir("thumb-names");
        let t = Thumbnails::new(d.join("thumbs"));
        let folder = d.join("thumbs");
        std::fs::create_dir_all(&folder).unwrap();
        for name in [
            "0123456789abcdef.png",
            "FEDCBA9876543210.png",
            "short.png",
            "0123456789abcdef.png.bak",
            ".0123456789abcdef.png.tmp",
        ] {
            std::fs::write(folder.join(name), b"x").unwrap();
        }
        t.sweep(&RecentFiles::new(5)).unwrap();
        assert!(!folder.join("0123456789abcdef.png").exists());
        assert!(!folder.join(".0123456789abcdef.png.tmp").exists());
        for kept in [
            "FEDCBA9876543210.png",
            "short.png",
            "0123456789abcdef.png.bak",
        ] {
            assert!(folder.join(kept).exists(), "{kept} is not a thumbnail");
        }
    }

    #[test]
    fn put_get_remove_round_trip_with_hashed_names() {
        let d = dir("thumb-roundtrip");
        let t = Thumbnails::new(d.join("thumbs"));
        let path = PathBuf::from("/home/someone/secret/report.arc");
        let image = png(64, 48, 100);
        t.put(&path, &image).unwrap();
        assert_eq!(t.get(&path), Some(image));
        let names: Vec<String> = std::fs::read_dir(d.join("thumbs"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(
            names[0].ends_with(".png") && names[0].len() == 20,
            "{names:?}"
        );
        assert!(!names[0].contains("report") && !names[0].contains("secret"));
        t.remove(&path).unwrap();
        assert_eq!(t.get(&path), None);
        t.remove(&path).unwrap();
    }

    #[test]
    fn bounds_and_format_are_checked_on_put() {
        let d = dir("thumb-bounds");
        let t = Thumbnails::new(d.join("thumbs"));
        let p = Path::new("/d/a.arc");
        let big_bytes = png(16, 16, MAX_THUMBNAIL_BYTES + 1);
        assert_eq!(
            t.put(p, &big_bytes).unwrap_err().code(),
            "app.thumbnail.too_large"
        );
        let wide = png(MAX_THUMBNAIL_SIDE + 1, 16, 64);
        assert_eq!(
            t.put(p, &wide).unwrap_err().code(),
            "app.thumbnail.too_large"
        );
        assert_eq!(
            t.put(p, b"GIF89a....").unwrap_err().code(),
            "app.thumbnail.invalid"
        );
        let zero = png(0, 16, 64);
        assert_eq!(t.put(p, &zero).unwrap_err().code(), "app.thumbnail.invalid");
        let edge = png(MAX_THUMBNAIL_SIDE, MAX_THUMBNAIL_SIDE, 64);
        t.put(p, &edge).unwrap();
        assert_eq!(t.get(p), Some(edge));
    }

    #[test]
    fn missing_or_damaged_files_are_simply_absent() {
        let d = dir("thumb-absent");
        let t = Thumbnails::new(d.join("thumbs"));
        let p = Path::new("/d/a.arc");
        assert_eq!(t.get(p), None);
        std::fs::create_dir_all(d.join("thumbs")).unwrap();
        std::fs::write(
            d.join("thumbs").join(file_name(p)),
            vec![0u8; MAX_THUMBNAIL_BYTES + 1],
        )
        .unwrap();
        assert_eq!(t.get(p), None, "oversize file");
        std::fs::write(
            d.join("thumbs").join(file_name(p)),
            b"not a png at all, really",
        )
        .unwrap();
        assert_eq!(t.get(p), None, "not a PNG");
    }

    #[test]
    fn sweep_removes_thumbnails_of_entries_no_longer_listed() {
        let d = dir("thumb-sweep");
        let t = Thumbnails::new(d.join("thumbs"));
        let mut list = RecentFiles::new(5);
        for n in ["a", "b", "c"] {
            let p = PathBuf::from(format!("/d/{n}.arc"));
            t.put(&p, &png(8, 8, 40)).unwrap();
            list.touch(&p);
        }
        std::fs::write(d.join("thumbs").join("notes.txt"), b"kept").unwrap();
        list.remove(Path::new("/d/a.arc")).unwrap();
        t.sweep(&list).unwrap();
        assert_eq!(t.get(Path::new("/d/a.arc")), None);
        assert!(t.get(Path::new("/d/b.arc")).is_some());
        assert!(t.get(Path::new("/d/c.arc")).is_some());
        assert!(d.join("thumbs").join("notes.txt").exists());
        t.sweep(&RecentFiles::new(5)).unwrap();
        assert_eq!(t.get(Path::new("/d/b.arc")), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dir;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/d/{name}"))
    }

    fn names(list: &RecentFiles) -> Vec<String> {
        list.iter()
            .map(|e| {
                format!(
                    "{}{}",
                    e.path.file_name().unwrap().to_string_lossy(),
                    if e.pinned { "*" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn most_recent_first_and_touch_moves_to_front() {
        let mut l = RecentFiles::new(5);
        for n in ["a", "b", "c"] {
            l.touch(&p(n));
        }
        l.touch(&p("a"));
        assert_eq!(names(&l), ["a", "c", "b"]);
    }

    #[test]
    fn list_is_bounded_and_drops_the_oldest_unpinned() {
        let mut l = RecentFiles::new(3);
        l.touch(&p("a"));
        l.pin(&p("a")).unwrap();
        for n in ["b", "c", "d", "e"] {
            l.touch(&p(n));
        }
        assert_eq!(names(&l), ["a*", "e", "d", "c"]);
        l.touch(&p("a"));
        assert_eq!(names(&l), ["a*", "e", "d", "c"], "touching keeps the pin");
    }

    #[test]
    fn pin_and_unpin() {
        let mut l = RecentFiles::new(3);
        for n in ["a", "b", "c"] {
            l.touch(&p(n));
        }
        l.pin(&p("a")).unwrap();
        l.touch(&p("d"));
        assert_eq!(names(&l), ["a*", "d", "c", "b"]);
        l.unpin(&p("a")).unwrap();
        // Unpinned, "a" is the oldest of four unpinned entries and is evicted.
        assert_eq!(names(&l), ["d", "c", "b"]);
        assert_eq!(l.pin(&p("a")).unwrap_err().code(), "app.recent.not_found");
        assert_eq!(
            l.unpin(&p("zz")).unwrap_err().code(),
            "app.recent.not_found"
        );
        let mut many = RecentFiles::new(MAX_ENTRIES);
        for i in 0..=MAX_PINNED {
            many.touch(&p(&i.to_string()));
        }
        for i in 0..MAX_PINNED {
            many.pin(&p(&i.to_string())).unwrap();
        }
        assert_eq!(
            many.pin(&p(&MAX_PINNED.to_string())).unwrap_err().code(),
            "app.recent.too_many_pinned"
        );
        many.pin(&p("0")).unwrap(); // already pinned: not an error
    }

    #[test]
    fn missing_files_are_removed_only_on_request() {
        let mut l = RecentFiles::new(5);
        for n in ["keep", "gone", "pinned-gone"] {
            l.touch(&p(n));
        }
        l.pin(&p("pinned-gone")).unwrap();
        let before = l.len();
        // Ordinary operations never check the file system.
        l.touch(&p("keep"));
        assert_eq!(l.len(), before);
        let removed = l.prune_missing(|path| path.ends_with("keep"));
        assert_eq!(removed.len(), 2);
        assert_eq!(names(&l), ["keep"]);
        l.remove(&p("keep")).unwrap();
        assert!(l.is_empty());
        assert_eq!(
            l.remove(&p("keep")).unwrap_err().code(),
            "app.recent.not_found"
        );
    }

    #[test]
    fn persistence_round_trips_and_tolerates_damage() {
        let d = dir("recent");
        let file = d.join("recent.toml");
        let mut l = RecentFiles::new(5);
        for n in ["a", "b", "c"] {
            l.touch(&p(n));
        }
        l.pin(&p("b")).unwrap();
        l.save(&file).unwrap();
        let (back, aside) = RecentFiles::load(&file, 5).unwrap();
        assert!(aside.is_none());
        assert_eq!(names(&back), names(&l));
        let junk: Table =
            "entries = [1, { path = 2 }, { path = \"/d/x\" }, { path = \"/d/x\", pinned = true }]"
                .parse()
                .unwrap();
        assert_eq!(names(&RecentFiles::from_table(&junk, 5)), ["x"]);
        std::fs::write(&file, "entries = [").unwrap();
        let (empty, aside) = RecentFiles::load(&file, 5).unwrap();
        assert!(empty.is_empty());
        assert_eq!(aside.unwrap().problem.code(), "app.settings_file.malformed");
    }

    #[test]
    fn stored_lists_are_bounded_on_load() {
        let mut text = String::from("entries = [");
        for i in 0..40 {
            text.push_str(&format!(
                "{{ path = \"/d/{i}\", pinned = {} }},",
                i % 2 == 0
            ));
        }
        text.push(']');
        let l = RecentFiles::from_table(&text.parse().unwrap(), 4);
        assert_eq!(l.iter().filter(|e| !e.pinned).count(), 4);
        assert_eq!(l.iter().filter(|e| e.pinned).count(), 20);
    }
}
