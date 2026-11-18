//! Settings files (CORE-APP-012): TOML tables on disk.
//!
//! A file is read into a [`toml::Table`], so keys the registry does not know survive a load and
//! save unchanged. Comments and formatting are not preserved; tables are written with sorted
//! keys. A file that cannot be used is renamed aside, never overwritten, and reported.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use arc_base::{Error, Result};
use toml::Table;

use crate::codes;

/// The largest settings file that is read, in bytes.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// The most aside names tried for one file.
const MAX_ASIDE: u32 = 10_000;

/// A file that was renamed aside.
#[derive(Debug)]
pub struct SetAside {
    /// Where the file now is: `<name>.malformed-<n>` beside the original.
    pub moved_to: PathBuf,
    /// The stable code and cause: `app.settings_file.malformed` or
    /// `app.settings_file.too_large`.
    pub problem: Error,
}

/// The outcome of [`load`].
#[derive(Debug)]
pub struct Loaded {
    /// The table read; empty when the file is missing or was set aside.
    pub table: Table,
    /// Set when the file was set aside and defaults apply.
    pub set_aside: Option<SetAside>,
}

/// Reads a settings file. A missing file yields an empty table. A file that is too large, not
/// UTF-8 or not valid TOML is renamed to the first free `<name>.malformed-<n>` and reported.
///
/// # Errors
/// `app.settings_file.read_failed` when the file exists but cannot be read, and
/// `app.settings_file.set_aside_failed` when a malformed file cannot be renamed.
pub fn load(path: &Path) -> Result<Loaded> {
    let read_failed = |e: std::io::Error| {
        Error::new(codes::FILE_READ_FAILED)
            .with_arg("path", path.display().to_string())
            .with_source(e)
    };
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded {
                table: Table::new(),
                set_aside: None,
            });
        }
        Err(e) => return Err(read_failed(e)),
    };
    let problem = if meta.len() > MAX_FILE_BYTES {
        Error::new(codes::FILE_TOO_LARGE)
            .with_detail("limit", i64::try_from(MAX_FILE_BYTES).unwrap_or(i64::MAX))
    } else {
        let bytes = fs::read(path).map_err(read_failed)?;
        match std::str::from_utf8(&bytes).map(str::parse::<Table>) {
            Ok(Ok(table)) => {
                return Ok(Loaded {
                    table,
                    set_aside: None,
                });
            }
            Ok(Err(e)) => Error::new(codes::FILE_MALFORMED).with_source(e),
            Err(e) => Error::new(codes::FILE_MALFORMED).with_source(e),
        }
    };
    let moved_to = set_aside(path)?;
    let problem = problem.with_arg("path", path.display().to_string());
    Ok(Loaded {
        table: Table::new(),
        set_aside: Some(SetAside { moved_to, problem }),
    })
}

fn set_aside(path: &Path) -> Result<PathBuf> {
    let fail = |e: Option<std::io::Error>| {
        let error =
            Error::new(codes::FILE_SET_ASIDE_FAILED).with_arg("path", path.display().to_string());
        match e {
            Some(e) => error.with_source(e),
            None => error,
        }
    };
    let name = path
        .file_name()
        .ok_or_else(|| fail(None))?
        .to_string_lossy();
    for n in 1..=MAX_ASIDE {
        let target = path.with_file_name(format!("{name}.malformed-{n}"));
        // shortcut: exists-then-rename is not atomic; a concurrent writer of the same aside name
        // could be replaced. Only one process owns a settings file, so upgrade if that changes.
        if target.symlink_metadata().is_ok() {
            continue;
        }
        return fs::rename(path, &target)
            .map(|()| target)
            .map_err(|e| fail(Some(e)));
    }
    Err(fail(None))
}

/// Writes a table atomically: to a temporary file beside `path`, flushed to disk, then renamed
/// over `path`. A failure leaves the previous file untouched and removes the temporary file.
///
/// # Errors
/// `app.settings_file.write_failed`.
pub fn save(path: &Path, table: &Table) -> Result<()> {
    let failed =
        || Error::new(codes::FILE_WRITE_FAILED).with_arg("path", path.display().to_string());
    let text = toml::to_string(table).map_err(|e| failed().with_source(e))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| failed().with_source(e))?;
    }
    let name = path.file_name().ok_or_else(failed)?.to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        failed().with_source(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::dir;

    #[test]
    fn missing_file_is_empty() {
        let d = dir("missing");
        let l = load(&d.join("user.toml")).unwrap();
        assert!(l.table.is_empty() && l.set_aside.is_none());
    }

    #[test]
    fn unknown_keys_survive_a_round_trip() {
        let d = dir("roundtrip");
        let p = d.join("sub").join("user.toml");
        let text = "future = { nested = [1, 2, 3] }\n[ui]\nlanguage = \"tr\"\nnew_key = 1.5\n";
        save(&p, &text.parse().unwrap()).unwrap();
        let first = load(&p).unwrap();
        assert!(first.set_aside.is_none());
        assert_eq!(first.table["future"]["nested"].as_array().unwrap().len(), 3);
        assert_eq!(first.table["ui"]["new_key"].as_float(), Some(1.5));
        save(&p, &first.table).unwrap();
        assert_eq!(load(&p).unwrap().table, first.table);
        assert!(!d.join("sub").join(".user.toml.tmp").exists());
    }

    #[test]
    fn malformed_file_is_set_aside_with_a_unique_name_and_reported() {
        let d = dir("malformed");
        let p = d.join("user.toml");
        for n in 1..=3 {
            fs::write(&p, format!("not = [valid {n}")).unwrap();
            let l = load(&p).unwrap();
            let aside = l.set_aside.unwrap();
            assert!(l.table.is_empty());
            assert_eq!(aside.problem.code(), "app.settings_file.malformed");
            assert_eq!(aside.moved_to, d.join(format!("user.toml.malformed-{n}")));
            assert!(!p.exists());
        }
        for n in 1..=3 {
            let kept = fs::read_to_string(d.join(format!("user.toml.malformed-{n}"))).unwrap();
            assert!(kept.ends_with(&format!("valid {n}")));
        }
    }

    #[test]
    fn invalid_utf8_and_oversized_files_are_set_aside() {
        let d = dir("badbytes");
        let p = d.join("user.toml");
        fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            load(&p).unwrap().set_aside.unwrap().problem.code(),
            "app.settings_file.malformed"
        );
        fs::write(&p, vec![b'#'; usize::try_from(MAX_FILE_BYTES).unwrap() + 1]).unwrap();
        assert_eq!(
            load(&p).unwrap().set_aside.unwrap().problem.code(),
            "app.settings_file.too_large"
        );
    }

    #[test]
    fn failed_save_keeps_the_old_file() {
        let d = dir("failsave");
        let p = d.join("user.toml");
        save(&p, &"a = 1".parse().unwrap()).unwrap();
        // The temporary name is taken by a directory, so creating it fails.
        fs::create_dir(d.join(".user.toml.tmp")).unwrap();
        let e = save(&p, &"a = 2".parse().unwrap()).unwrap_err();
        assert_eq!(e.code(), "app.settings_file.write_failed");
        assert_eq!(load(&p).unwrap().table["a"].as_integer(), Some(1));
    }
}
