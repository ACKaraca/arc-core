//! The package schema and the open sequence (CORE-DOC-040): a created package matches the
//! documented schema statement for statement; newer format majors are refused without touching
//! the file, newer minors open read-only; files that are not packages and damaged packages fail
//! with stable codes.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::path::Path;
use std::sync::Arc;

use arc_base::{Limits, Result};
use arc_doc::LoadOptions;
use arc_doc::error::codes;
use arc_doc::package::{
    Access, Contents, FORMAT, FormatVersion, Metadata, Mode, Package, ReadOnlyCause, lock_path,
    write,
};
use package_support::{TempDir, owner, registry, stored, types};
use rusqlite::Connection;

const DOCUMENT: &str = include_str!("../../../docs/architecture/document-package.md");

/// The statements of the documented format 1.0 schema.
fn documented() -> Vec<String> {
    let section = DOCUMENT.split("### Format 1.0 schema").nth(1).unwrap();
    let block = section
        .split("```sql\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    block
        .split(";\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn create(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path(name);
    let meta = Metadata::default();
    let declared = types(false);
    let rows = [stored(3, 1, 1, None, 1)];
    let contents = Contents {
        metadata: &meta,
        types: &declared,
        objects: &rows,
        previews: &[],
        snapshot: None,
    };
    write(&path, &contents, &mut |_| Ok(())).unwrap();
    path
}

fn edit(path: &Path, sql: &str) {
    Connection::open(path).unwrap().execute_batch(sql).unwrap();
}

fn code<T: std::fmt::Debug>(result: Result<T>) -> &'static str {
    result.unwrap_err().code()
}

#[test]
fn a_created_package_has_exactly_the_documented_schema() {
    let dir = TempDir::new("schema");
    let path = dir.path("new.arc");
    let package = Package::create(&path, &Metadata::default(), &owner("u", "h")).unwrap();
    assert_eq!(package.format(), FormatVersion { major: 1, minor: 0 });
    assert_eq!(package.access(), &Access::Edit);
    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let statements: Vec<String> = conn
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(statements, documented());
    let automatic: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE sql IS NULL")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(automatic, vec!["sqlite_autoindex_arc_journal_1".to_owned()]);
    let pragma = |name: &str| -> rusqlite::types::Value {
        conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .unwrap()
    };
    use rusqlite::types::Value::{Integer, Text};
    assert_eq!(pragma("application_id"), Integer(0x4152_4331));
    assert_eq!(pragma("user_version"), Integer(1));
    assert_eq!(pragma("page_size"), Integer(4096));
    assert_eq!(pragma("encoding"), Text("UTF-8".into()));
    assert_eq!(pragma("journal_mode"), Text("delete".into()));
    assert_eq!(pragma("auto_vacuum"), Integer(0));
    let format: String = conn
        .query_row(
            "SELECT value FROM arc_meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(format, FORMAT.to_string());
    // A second create of the same path is refused.
    drop(package);
    assert_eq!(
        code(Package::create(
            &path,
            &Metadata::default(),
            &owner("u", "h")
        )),
        codes::PACKAGE_EXISTS.code
    );
}

#[test]
fn a_newer_major_format_is_refused_without_touching_the_file() {
    let dir = TempDir::new("newer-major");
    let path = create(&dir, "newer.arc");
    edit(
        &path,
        "PRAGMA user_version = 2; UPDATE arc_meta SET value = '2.0' WHERE key = 'format_version';",
    );
    let bytes = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let files = dir.files();
    for mode in [Mode::ReadOnly, Mode::Edit(&owner("u", "h"))] {
        let error = Package::open(&path, mode, &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), codes::PACKAGE_NEWER_FORMAT.code);
        assert_eq!(error.arguments().get("version"), Some(&"2".into()));
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(dir.files(), files);
    assert!(!lock_path(&path).exists());
}

#[test]
fn a_newer_minor_format_opens_read_only_without_a_lock() {
    let dir = TempDir::new("newer-minor");
    let path = create(&dir, "minor.arc");
    edit(
        &path,
        "UPDATE arc_meta SET value = '1.7' WHERE key = 'format_version'; CREATE TABLE arc_new (x);",
    );
    let package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let minor = FormatVersion { major: 1, minor: 7 };
    assert_eq!(
        package.access(),
        &Access::ReadOnly(ReadOnlyCause::NewerFormat(minor))
    );
    assert!(!lock_path(&path).exists());
    let loaded = package
        .load(registry(true), &LoadOptions::default())
        .unwrap();
    assert_eq!(loaded.snapshot.len(), 1);
}

#[test]
fn files_that_are_not_packages_are_refused() {
    let dir = TempDir::new("not");
    let text = dir.path("text.arc");
    std::fs::write(
        &text,
        b"not a database at all, long enough to fill the header of sqlite files; \
        padding padding padding padding padding",
    )
    .unwrap();
    let empty = dir.path("empty.arc");
    std::fs::write(&empty, b"").unwrap();
    let other = dir.path("other.db");
    edit(&other, "CREATE TABLE t (x);");
    let no_meta = create(&dir, "nometa.arc");
    edit(&no_meta, "DROP TABLE arc_meta;");
    for path in [&text, &empty, &other, &no_meta] {
        assert_eq!(
            code(Package::open(path, Mode::ReadOnly, &Limits::DEFAULT)),
            codes::PACKAGE_NOT_A_PACKAGE.code,
            "{}",
            path.display()
        );
    }
    assert_eq!(
        code(Package::open(
            &dir.path("missing.arc"),
            Mode::ReadOnly,
            &Limits::DEFAULT
        )),
        codes::PACKAGE_IO_FAILED.code
    );
}

#[test]
fn damaged_packages_fail_with_stable_codes() {
    let dir = TempDir::new("damaged");
    let open = |path: &Path| Package::open(path, Mode::ReadOnly, &Limits::DEFAULT);
    let load =
        |path: &Path| open(path).and_then(|p| p.load(registry(true), &LoadOptions::default()));
    let cases: [(&str, &str, &str, bool); 7] = [
        (
            "table",
            "DROP TABLE arc_preview;",
            codes::PACKAGE_SCHEMA_INVALID.code,
            false,
        ),
        (
            "format",
            "UPDATE arc_meta SET value = 'one' WHERE key = 'format_version';",
            codes::PACKAGE_SCHEMA_INVALID.code,
            false,
        ),
        (
            "major",
            "UPDATE arc_meta SET value = '0.4' WHERE key = 'format_version';",
            codes::PACKAGE_SCHEMA_INVALID.code,
            false,
        ),
        (
            "meta",
            "INSERT INTO arc_meta VALUES ('document_id', 'nope');",
            codes::PACKAGE_META_INVALID.code,
            false,
        ),
        (
            "id",
            "INSERT INTO arc_object VALUES ('text id', 't.a', 3, 1, NULL, NULL, NULL, 0, 0, x'00', x'00', x'00');",
            codes::PACKAGE_VALUE_INVALID.code,
            true,
        ),
        (
            "utf8",
            "UPDATE arc_object SET name = CAST(x'ff' AS TEXT);",
            codes::PACKAGE_VALUE_INVALID.code,
            true,
        ),
        (
            "type",
            "INSERT INTO arc_type VALUES ('t.c', 'one', 0, '');",
            codes::PACKAGE_VALUE_INVALID.code,
            true,
        ),
    ];
    for (name, sql, expected, at_load) in cases {
        let path = create(&dir, &format!("{name}.arc"));
        edit(&path, sql);
        let found = if at_load {
            code(load(&path))
        } else {
            code(open(&path))
        };
        assert_eq!(found, expected, "{name}");
    }
    // A value beyond `input_bytes` is refused when it is read.
    let path = create(&dir, "big.arc");
    edit(&path, "UPDATE arc_object SET payload = zeroblob(100000);");
    let small = LoadOptions {
        limits: Limits {
            input_bytes: 50_000,
            ..Limits::DEFAULT
        },
        ..LoadOptions::default()
    };
    let error = open(&path)
        .unwrap()
        .load(Arc::clone(&registry(true)), &small)
        .unwrap_err();
    assert_eq!(error.code(), codes::LOAD_LIMIT_EXCEEDED.code);
    // Too many rows fail the load as a whole.
    let few = LoadOptions {
        limits: Limits {
            record_count: 1,
            ..Limits::DEFAULT
        },
        ..LoadOptions::default()
    };
    let error = open(&path).unwrap().load(registry(true), &few).unwrap_err();
    assert_eq!(error.code(), codes::LOAD_LIMIT_EXCEEDED.code);
}
