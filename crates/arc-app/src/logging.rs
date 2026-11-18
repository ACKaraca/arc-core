//! Structured logging to size-rotated files (CORE-APP-015).
//!
//! [`subscriber`] builds a `tracing` subscriber that writes one text record per event, with its
//! fields as `key=value`, to a [`RotatingWriter`]. Document content and credentials must not be
//! logged: wrap any such value in [`Redacted`], whose formatting never touches the wrapped value.
//! This crate has no log viewer.

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
    file: File,
    size: u64,
}

impl State {
    fn numbered(&self, n: u32) -> PathBuf {
        self.dir.join(format!("{}.{n}", self.name))
    }

    fn open(path: &Path) -> io::Result<(File, u64)> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok((file, size))
    }

    /// Shifts `name` to `name.1`, `name.1` to `name.2`, and so on; the file that would become
    /// `name.<max_files>` is deleted, so at most `max_files` files exist.
    fn rotate(&mut self) -> io::Result<()> {
        let current = self.dir.join(&self.name);
        if self.max_files > 1 {
            let _ = fs::remove_file(self.numbered(self.max_files - 1));
            for n in (1..self.max_files - 1).rev() {
                let from = self.numbered(n);
                if from.exists() {
                    fs::rename(from, self.numbered(n + 1))?;
                }
            }
            fs::rename(&current, self.numbered(1))?;
        } else {
            fs::remove_file(&current)?;
        }
        let (file, size) = Self::open(&current)?;
        self.file = file;
        self.size = size;
        Ok(())
    }

    fn write(&mut self, record: &[u8]) -> io::Result<()> {
        let len = u64::try_from(record.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(len) > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(record)?;
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
        let (file, size) = State::open(&config.dir.join(&config.file_name)).map_err(io)?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                dir: config.dir.clone(),
                name: config.file_name.clone(),
                max_bytes: config.max_file_bytes,
                max_files: config.max_files,
                file,
                size,
            })),
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
        state.file.flush()
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
