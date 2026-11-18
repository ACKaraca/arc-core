//! The recent-files model (CORE-APP-007): a bounded, most-recent-first list with pinned entries.
//! Thumbnails are not part of this model.
//!
//! Paths are compared as given; the caller normalizes them. The list is persisted as its own TOML
//! file through [`settings_file`](crate::settings_file), so a malformed file is set aside the same
//! way. Entries are never removed because a file is missing unless [`RecentFiles::prune_missing`]
//! is called.

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
