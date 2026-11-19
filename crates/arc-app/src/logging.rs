//! Structured logging to size-rotated files (CORE-APP-015).
//!
//! [`subscriber`] builds a `tracing` subscriber that writes one text record per event, with its
//! fields as `key=value`, to a [`RotatingWriter`]. Document content and credentials must not be
//! logged: wrap any such value in [`Redacted`], whose formatting never touches the wrapped value.
//! [`subscriber_with_ring`] also keeps the most recent records in a bounded [`LogRing`] for the
//! shell's log viewer; the ring holds the same formatted text as the files, so a redacted value
//! is redacted there too.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use arc_base::{Error, Result};
use tracing::Level;

use crate::codes;
use crate::settings::{SettingValue, SettingsStore};

/// The text a [`Redacted`] value formats as.
pub const REDACTED: &str = "[redacted]";

/// Marks a value that must never reach a log line. Both `Debug` and `Display` print
/// [`REDACTED`] without reading the value, and the type has no other formatting.
///
/// ```
/// use arc_app::logging::Redacted;
/// assert_eq!(format!("{:?} {}", Redacted("secret"), Redacted(42)), "[redacted] [redacted]");
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Redacted<T>(pub T);

impl<T> std::fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> std::fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

/// The smallest accepted file size limit.
pub const MIN_FILE_BYTES: u64 = 4096;
/// The largest accepted file size limit.
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// The largest accepted file count.
pub const MAX_FILES: u32 = 100;

/// Logging configuration.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// The directory of the log files; created when missing.
    pub dir: PathBuf,
    /// The name of the current file, for example `arc.log`; rotated files append `.1`, `.2`, ...
    pub file_name: String,
    /// The most verbose level that is written.
    pub level: Level,
    /// The size at which the current file is rotated, in bytes.
    pub max_file_bytes: u64,
    /// The number of files kept, the current one included.
    pub max_files: u32,
}

impl LogConfig {
    /// Reads `log.level`, `log.max_file_bytes` and `log.max_files` from a store built over
    /// [`SettingsRegistry::builtin`](crate::settings::SettingsRegistry::builtin).
    ///
    /// # Errors
    /// `app.log.invalid_config` when a setting is missing from the registry or out of range.
    pub fn from_settings(dir: PathBuf, file_name: &str, store: &SettingsStore) -> Result<Self> {
        let bad =
            |key: &str| Error::new(codes::LOG_INVALID_CONFIG).with_arg("field", key.to_owned());
        let level = match store.get("log.level").map_err(|_| bad("log.level"))?.value {
            SettingValue::Text(t) => t.parse().map_err(|_| bad("log.level"))?,
            _ => return Err(bad("log.level")),
        };
        let int = |key: &str| match store.get(key).map_err(|_| bad(key))?.value {
            SettingValue::Int(v) => u64::try_from(v).map_err(|_| bad(key)),
            _ => Err(bad(key)),
        };
        let config = Self {
            dir,
            file_name: file_name.to_owned(),
            level,
            max_file_bytes: int("log.max_file_bytes")?,
            max_files: u32::try_from(int("log.max_files")?).map_err(|_| bad("log.max_files"))?,
        };
        config.validate()?;
        Ok(config)
    }

    /// Checks the limits and the file name.
    ///
    /// # Errors
    /// `app.log.invalid_config`, with the offending `field` as argument.
    pub fn validate(&self) -> Result<()> {
        let bad = |field: &str| {
            Err(Error::new(codes::LOG_INVALID_CONFIG).with_arg("field", field.to_owned()))
        };
        if !(MIN_FILE_BYTES..=MAX_FILE_BYTES).contains(&self.max_file_bytes) {
            return bad("max_file_bytes");
        }
        if !(1..=MAX_FILES).contains(&self.max_files) {
            return bad("max_files");
        }
        let name = &self.file_name;
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return bad("file_name");
        }
        Ok(())
    }
}

struct State {
    dir: PathBuf,
    name: String,
    max_bytes: u64,
    max_files: u32,
    /// The open current file; `None` after a rotation closed it, until [`State::current`]
    /// opens it again.
    file: Option<File>,
    size: u64,
}

impl State {
    fn numbered(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{}.{n}", self.name))
    }

    /// The current file, opened for appending when it is not open.
    fn current(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(&self.name))?;
            self.size = file.metadata()?.len();
            self.file = Some(file);
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is not open"))
    }

    /// Rotates with the operating system's rename: see [`State::shift_with`].
    fn shift(&mut self) -> io::Result<()> {
        self.shift_with(|from, to| fs::rename(from, to))
    }

    /// Moves the current file to `name.1`, `name.1` to `name.2`, and so on, so that the file
    /// that held `name.<max_files - 1>` is dropped. The dropped file moves to `name.<max_files>`
    /// first and is removed last. Every move is recorded, and when one fails the moves are undone
    /// in reverse order, so a failed rotation restores the files exactly and a retry starts from
    /// the same state. `rename` is injected so tests can block a step.
    fn shift_with(&self, rename: impl Fn(&Path, &Path) -> io::Result<()>) -> io::Result<()> {
        let mut moved = Vec::new();
        let result = self.shift_steps(&rename, &mut moved);
        if result.is_err() {
            for (from, to) in moved.iter().rev() {
                let _ = rename(to, from);
            }
        }
        result
    }

    fn shift_steps(
        &self,
        rename: &impl Fn(&Path, &Path) -> io::Result<()>,
        moved: &mut Vec<(PathBuf, PathBuf)>,
    ) -> io::Result<()> {
        let mut mv = |from: PathBuf, to: PathBuf| -> io::Result<()> {
            rename(&from, &to)?;
            moved.push((from, to));
            Ok(())
        };
        let current = self.dir.join(&self.name);
        let doomed = self.numbered(self.max_files);
        if self.max_files == 1 {
            mv(current, doomed.clone())?;
        } else {
            let oldest = self.numbered(self.max_files - 1);
            if oldest.exists() {
                mv(oldest, doomed.clone())?;
            }
            for n in (1..self.max_files - 1).rev() {
                let from = self.numbered(n);
                if from.exists() {
                    mv(from, self.numbered(n + 1))?;
                }
            }
            mv(current, self.numbered(1))?;
        }
        // The commit: the dropped file is removed only after every move succeeded.
        if doomed.exists() {
            fs::remove_file(&doomed)?;
        }
        Ok(())
    }

    /// Rotates and reopens the current file. Rotation is attempted at a later write when this
    /// fails: the current file is open for appending again, so no record is lost.
    fn rotate(&mut self) -> io::Result<()> {
        // std opens files without delete sharing on Windows, which makes a rename of a file
        // with an open handle fail, so the handle is closed before the shift.
        self.file = None;
        let shifted = self.shift();
        let reopened = self.current().map(drop);
        shifted.and(reopened)
    }

    fn write(&mut self, record: &[u8]) -> io::Result<()> {
        let len = u64::try_from(record.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(len) > self.max_bytes {
            // A failed rotation (a viewer holds a file on Windows, for example) keeps the
            // record in the current file; the next write tries again.
            let _ = self.rotate();
        }
        self.current()?.write_all(record)?;
        self.size = self.size.saturating_add(len);
        Ok(())
    }
}

/// A file writer that rotates by size. Each `write` call is one record and is never split across
/// files, so a file stays within the size limit unless a single record exceeds it. Clones share
/// the same files.
#[derive(Clone)]
pub struct RotatingWriter {
    state: Arc<Mutex<State>>,
}

impl RotatingWriter {
    /// Opens the current file for appending, creating the directory when needed.
    ///
    /// # Errors
    /// `app.log.invalid_config` or `app.log.io`.
    pub fn open(config: &LogConfig) -> Result<Self> {
        config.validate()?;
        let io = |e: io::Error| Error::new(codes::LOG_IO).with_source(e);
        fs::create_dir_all(&config.dir).map_err(io)?;
        let mut state = State {
            dir: config.dir.clone(),
            name: config.file_name.clone(),
            max_bytes: config.max_file_bytes,
            max_files: config.max_files,
            file: None,
            size: 0,
        };
        state.current().map_err(io)?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
        })
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.write(buf).map(|()| buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Builds the subscriber: records at or above `config.level`, without colors, written to the
/// rotating files. Installing it (`tracing::subscriber::set_global_default`) is the caller's
/// decision.
///
/// # Errors
/// As [`RotatingWriter::open`].
pub fn subscriber(config: &LogConfig) -> Result<impl tracing::Subscriber + Send + Sync + 'static> {
    let writer = RotatingWriter::open(config)?;
    Ok(tracing_subscriber::fmt()
        .with_max_level(config.level)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish())
}

/// The most records a [`LogRing`] keeps.
pub const MAX_RING_RECORDS: usize = 1000;
/// The most bytes of one record a [`LogRing`] keeps; the rest is cut at a character boundary.
pub const MAX_RING_RECORD_BYTES: usize = 4096;

/// The most recent log records as formatted text, oldest first, at most [`MAX_RING_RECORDS`] of
/// at most [`MAX_RING_RECORD_BYTES`] each. Clones share the records.
#[derive(Clone, Default)]
pub struct LogRing {
    records: Arc<Mutex<VecDeque<String>>>,
}

impl std::fmt::Debug for LogRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRing").finish_non_exhaustive()
    }
}

impl LogRing {
    /// An empty ring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one record, dropping the oldest beyond [`MAX_RING_RECORDS`]. Invalid UTF-8 is
    /// replaced and a trailing line break removed.
    pub fn push(&self, record: &[u8]) {
        let text = String::from_utf8_lossy(record);
        let mut text = text.trim_end_matches(['\n', '\r']).to_owned();
        if text.len() > MAX_RING_RECORD_BYTES {
            let mut end = MAX_RING_RECORD_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        if records.len() >= MAX_RING_RECORDS {
            records.pop_front();
        }
        records.push_back(text);
    }

    /// The kept records, oldest first.
    #[must_use]
    pub fn records(&self) -> Vec<String> {
        self.range(0..usize::MAX)
    }

    /// The kept records at positions `range` (oldest is 0), clipped to the records kept.
    #[must_use]
    pub fn range(&self, range: std::ops::Range<usize>) -> Vec<String> {
        let records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let len = range.end.saturating_sub(range.start);
        records
            .iter()
            .skip(range.start)
            .take(len)
            .cloned()
            .collect()
    }

    /// The number of kept records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// True when no record is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Writes each record to the rotating files and to the ring.
#[derive(Clone)]
struct Tee {
    file: RotatingWriter,
    ring: LogRing,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ring.push(buf);
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// As [`subscriber`], and every record is also kept in `ring` for the log viewer.
///
/// # Errors
/// As [`RotatingWriter::open`].
pub fn subscriber_with_ring(
    config: &LogConfig,
    ring: LogRing,
) -> Result<impl tracing::Subscriber + Send + Sync + 'static> {
    let tee = Tee {
        file: RotatingWriter::open(config)?,
        ring,
    };
    Ok(tracing_subscriber::fmt()
        .with_max_level(config.level)
        .with_ansi(false)
        .with_writer(move || tee.clone())
        .finish())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testutil::dir;

    fn config(dir: PathBuf, max_file_bytes: u64, max_files: u32) -> LogConfig {
        LogConfig {
            dir,
            file_name: "arc.log".into(),
            level: Level::INFO,
            max_file_bytes,
            max_files,
        }
    }

    fn read_all(dir: &Path) -> String {
        let mut out = String::new();
        for entry in fs::read_dir(dir).unwrap() {
            out.push_str(&fs::read_to_string(entry.unwrap().path()).unwrap());
        }
        out
    }

    #[test]
    fn logs_rotate_at_the_configured_size_and_count() {
        let d = dir("rotate");
        let cfg = config(d.clone(), 4096, 3);
        tracing::subscriber::with_default(subscriber(&cfg).unwrap(), || {
            for i in 0..300 {
                tracing::info!(
                    index = i,
                    "a record of moderate length for the rotation test"
                );
            }
        });
        let mut names: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["arc.log", "arc.log.1", "arc.log.2"]);
        for name in &names {
            let size = fs::metadata(d.join(name)).unwrap().len();
            assert!((1..=4096).contains(&size), "{name}: {size}");
        }
        // Newest records are in the current file, older ones in higher numbers.
        let index_in = |name: &str| -> i32 {
            let text = fs::read_to_string(d.join(name)).unwrap();
            let last = text.lines().last().unwrap();
            last.split("index=")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        assert_eq!(index_in("arc.log"), 299);
        assert!(index_in("arc.log.1") < index_in("arc.log"));
        assert!(index_in("arc.log.2") < index_in("arc.log.1"));
    }

    #[test]
    fn a_single_file_is_truncated_when_full() {
        let d = dir("single");
        let mut w = RotatingWriter::open(&config(d.clone(), 4096, 1)).unwrap();
        for _ in 0..100 {
            w.write_all(&[b'x'; 100]).unwrap();
        }
        let names: Vec<_> = fs::read_dir(&d).unwrap().collect();
        assert_eq!(names.len(), 1);
        assert!(fs::metadata(d.join("arc.log")).unwrap().len() <= 4096);
    }

    #[test]
    fn a_failed_rotation_keeps_appending_and_a_later_write_retries() {
        let d = dir("retry");
        // A directory where the first rotated file goes makes the shift fail, as a file held by
        // another process does on Windows.
        let blocker = d.join("arc.log.1");
        fs::create_dir(&blocker).unwrap();
        let mut w = RotatingWriter::open(&config(d.clone(), 4096, 2)).unwrap();
        for _ in 0..8 {
            w.write_all(&[b'x'; 1000]).unwrap();
        }
        assert_eq!(fs::metadata(d.join("arc.log")).unwrap().len(), 8000);
        fs::remove_dir(&blocker).unwrap();
        w.write_all(&[b'y'; 1000]).unwrap();
        assert_eq!(fs::metadata(d.join("arc.log.1")).unwrap().len(), 8000);
        assert_eq!(fs::metadata(d.join("arc.log")).unwrap().len(), 1000);
        assert!(!d.join("arc.log.0").exists());
    }

    #[test]
    fn a_blocked_middle_step_loses_no_rotated_content_on_retry() {
        let d = dir("blocked");
        let state = State {
            dir: d.clone(),
            name: "arc.log".into(),
            max_bytes: 4096,
            max_files: 5,
            file: None,
            size: 0,
        };
        for (name, text) in [
            ("arc.log", "c"),
            ("arc.log.1", "1"),
            ("arc.log.2", "2"),
            ("arc.log.3", "3"),
            ("arc.log.4", "4"),
        ] {
            fs::write(d.join(name), text).unwrap();
        }
        let blocked = d.join("arc.log.2");
        let refuse = |from: &Path, to: &Path| {
            if from == blocked.as_path() {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fs::rename(from, to)
            }
        };
        assert!(state.shift_with(refuse).is_err());
        assert!(state.shift_with(refuse).is_err());
        let contents = |name: &str| fs::read_to_string(d.join(name)).ok();
        assert_eq!(contents("arc.log").as_deref(), Some("c"));
        assert_eq!(contents("arc.log.1").as_deref(), Some("1"));
        assert_eq!(contents("arc.log.2").as_deref(), Some("2"));
        assert_eq!(contents("arc.log.3").as_deref(), Some("3"));
        assert_eq!(contents("arc.log.4").as_deref(), Some("4"));
        state.shift_with(|from, to| fs::rename(from, to)).unwrap();
        // The oldest file is dropped by the completed rotation; the others moved up.
        assert_eq!(contents("arc.log.1").as_deref(), Some("c"));
        assert_eq!(contents("arc.log.2").as_deref(), Some("1"));
        assert_eq!(contents("arc.log.3").as_deref(), Some("2"));
        assert_eq!(contents("arc.log.4").as_deref(), Some("3"));
        assert!(!d.join("arc.log.5").exists());
        assert!(!d.join("arc.log").exists());
    }

    #[test]
    fn a_redacted_field_never_reaches_the_file() {
        let d = dir("redact");
        let secret = "SECRET-TOKEN-4711 document text";
        tracing::subscriber::with_default(subscriber(&config(d.clone(), 4096, 2)).unwrap(), || {
            tracing::info!(token = ?Redacted(secret), content = %Redacted(secret), "opened");
            let span = tracing::info_span!("save", body = ?Redacted(secret));
            span.in_scope(|| tracing::warn!(note = %Redacted(String::from(secret)), "saving"));
        });
        let text = read_all(&d);
        assert!(
            !text.contains("SECRET") && !text.contains("document text"),
            "{text}"
        );
        assert!(
            text.contains("token=[redacted]") && text.contains("content=[redacted]"),
            "{text}"
        );
    }

    #[test]
    fn the_ring_keeps_recent_redacted_records_within_bounds() {
        let d = dir("ring");
        let ring = LogRing::new();
        let sub = subscriber_with_ring(&config(d.clone(), 1 << 20, 2), ring.clone()).unwrap();
        tracing::subscriber::with_default(sub, || {
            tracing::info!(content = ?Redacted("SECRET body"), "opened");
            for i in 0..MAX_RING_RECORDS + 5 {
                tracing::info!(index = i, "filler");
            }
        });
        let records = ring.records();
        assert_eq!(records.len(), MAX_RING_RECORDS);
        assert!(
            records
                .iter()
                .all(|r| !r.contains("SECRET") && !r.ends_with('\n'))
        );
        assert!(records.last().unwrap().contains("index=1004"));
        ring.push("é".repeat(MAX_RING_RECORD_BYTES).as_bytes());
        assert!(ring.records().last().unwrap().len() <= MAX_RING_RECORD_BYTES);
    }

    #[test]
    fn level_filters_records() {
        let d = dir("level");
        tracing::subscriber::with_default(subscriber(&config(d.clone(), 4096, 2)).unwrap(), || {
            tracing::debug!("hidden-debug");
            tracing::warn!("shown-warning");
        });
        let text = read_all(&d);
        assert!(text.contains("shown-warning") && !text.contains("hidden-debug"));
    }

    #[test]
    fn configuration_is_validated() {
        let ok = config(PathBuf::from("x"), 4096, 1);
        assert!(ok.validate().is_ok());
        for bad in [
            LogConfig {
                max_file_bytes: 100,
                ..ok.clone()
            },
            LogConfig {
                max_file_bytes: MAX_FILE_BYTES + 1,
                ..ok.clone()
            },
            LogConfig {
                max_files: 0,
                ..ok.clone()
            },
            LogConfig {
                max_files: MAX_FILES + 1,
                ..ok.clone()
            },
            LogConfig {
                file_name: "a/b".into(),
                ..ok.clone()
            },
            LogConfig {
                file_name: String::new(),
                ..ok.clone()
            },
        ] {
            assert_eq!(bad.validate().unwrap_err().code(), "app.log.invalid_config");
        }
    }

    #[test]
    fn config_comes_from_settings() {
        use crate::settings::{Scope, SettingsRegistry};
        let mut store = SettingsStore::new(SettingsRegistry::builtin());
        store
            .set(Scope::User, "log.level", SettingValue::Text("debug".into()))
            .unwrap();
        store
            .set(Scope::User, "log.max_files", SettingValue::Int(7))
            .unwrap();
        let cfg = LogConfig::from_settings(PathBuf::from("x"), "arc.log", &store).unwrap();
        assert_eq!(
            (cfg.level, cfg.max_files, cfg.max_file_bytes),
            (Level::DEBUG, 7, 5 << 20)
        );
        let empty = SettingsStore::new(SettingsRegistry::new());
        let e = LogConfig::from_settings(PathBuf::from("x"), "arc.log", &empty).unwrap_err();
        assert_eq!(e.code(), "app.log.invalid_config");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn rotation_keeps_count_and_size_bounds(
            max_bytes in 4096u64..6000,
            max_files in 1u32..5,
            sizes in proptest::collection::vec(1usize..700, 1..60),
        ) {
            let d = dir("prop");
            let mut w = RotatingWriter::open(&config(d.clone(), max_bytes, max_files)).unwrap();
            for s in &sizes {
                w.write_all(&vec![b'x'; *s]).unwrap();
            }
            let files: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap()).collect();
            prop_assert!(files.len() <= usize::try_from(max_files).unwrap());
            for f in files {
                prop_assert!(f.metadata().unwrap().len() <= max_bytes);
            }
            let _ = fs::remove_dir_all(&d);
        }
    }
}
