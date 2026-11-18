//! Inspection and check (CORE-DOC-050): every fixture decodes through its stored descriptors
//! without a registry, the check finds each seeded corruption, and neither changes a byte of
//! the file.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::path::{Path, PathBuf};

use arc_base::Limits;
use arc_doc::error::codes;
use arc_doc::package::{check, inspect};
use package_support::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/packages")
        .join(name)
}

/// The hexadecimal identifier of fixture object `n`.
fn id_hex(n: u8) -> String {
    format!("01900000000070008000000000000{n:03x}")
}

fn id_blob(n: u8) -> Vec<u8> {
    let mut bytes = 0x0190_0000_0000_7000_8000_0000_0000_0000_u128
        .to_be_bytes()
        .to_vec();
    bytes[15] = n;
    bytes
}

/// The findings as (code, detail or argument `id`) pairs, sorted.
fn summary(findings: &[arc_base::Error]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = findings
        .iter()
        .map(|finding| {
            let id = finding
                .details()
                .get("id")
                .or_else(|| finding.arguments().get("id"))
                .map(|value| match value {
                    arc_base::error::Value::Text(text) => text.clone(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default();
            (finding.code().to_owned(), id)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Runs `f` on the file at `path` and asserts that it left every byte unchanged and created
/// no file beside it.
fn untouched<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let before = std::fs::read(path).unwrap();
    let dir = path.parent().unwrap();
    let listing = || {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let files = listing();
    let result = f();
    assert_eq!(std::fs::read(path).unwrap(), before, "{}", path.display());
    assert_eq!(listing(), files);
    result
}

#[test]
fn every_fixture_object_decodes_through_its_descriptor() {
    let minimal = inspect(&fixture("v1-minimal.arc"), &Limits::DEFAULT).unwrap();
    assert_eq!(minimal["format_version"], "1.0");
    assert_eq!(minimal["metadata"]["units"], "m");
    assert_eq!(minimal["statistics"]["objects"], 1);
    let point = &minimal["objects"][0];
    assert_eq!(point["type_key"], "fx.point");
    assert_eq!(point["values"]["x"], 1.5);
    assert_eq!(point["values"]["y"], -1.5);
    assert_eq!(point["values"]["label"], serde_json::Value::Null);
    assert_eq!(point["hash_matches"], true);

    let typical = inspect(&fixture("v1-typical.arc"), &Limits::DEFAULT).unwrap();
    let objects = typical["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 6);
    assert_eq!(typical["statistics"]["per_type"]["fx.point"]["objects"], 3);
    assert_eq!(typical["statistics"]["previews"], 2);
    for object in objects {
        match object["type_key"].as_str().unwrap() {
            // The rows the fixture holds as preserved (a payload its own descriptor of no
            // field does not describe) and quarantined (a damaged payload): reported, with the
            // code, not guessed.
            "fx.legacy" => assert_eq!(object["error"]["code"], "doc.payload.invalid"),
            "fx.point" if object["id"] == "01900000-0000-7000-8000-000000000006" => {
                assert_eq!(object["error"]["code"], "doc.payload.invalid");
            }
            _ => assert!(object["values"].is_object(), "{object}"),
        }
    }
    let line = objects.iter().find(|o| o["type_key"] == "fx.line").unwrap();
    assert_eq!(
        line["values"]["start"],
        "01900000-0000-7000-8000-000000000001"
    );
    let note = objects.iter().find(|o| o["type_key"] == "fx.note").unwrap();
    assert_eq!(note["version"], "2.0");
    assert_eq!(note["values"]["size"], 12);
    let types = typical["types"].as_array().unwrap();
    assert!(
        types.iter().all(|t| t["descriptor"].is_object()),
        "{types:?}"
    );
}

#[test]
fn a_newer_major_fixture_is_reported_as_the_reader_refuses_it() {
    let path = fixture("newer-major.arc");
    let error = untouched(&path, || inspect(&path, &Limits::DEFAULT).unwrap_err());
    assert_eq!(error.code(), codes::PACKAGE_NEWER_FORMAT.code);
    let error = untouched(&path, || check(&path, &Limits::DEFAULT).unwrap_err());
    assert_eq!(error.code(), codes::PACKAGE_NEWER_FORMAT.code);
}

#[test]
fn fixtures_check_clean_except_for_their_held_rows() {
    let minimal = fixture("v1-minimal.arc");
    assert!(untouched(&minimal, || check(&minimal, &Limits::DEFAULT).unwrap()).is_empty());
    let typical = fixture("v1-typical.arc");
    let findings = untouched(&typical, || check(&typical, &Limits::DEFAULT).unwrap());
    assert_eq!(
        summary(&findings),
        vec![
            ("doc.payload.invalid".to_owned(), id_hex(5)),
            ("doc.payload.invalid".to_owned(), id_hex(6)),
        ]
    );
}

#[test]
fn seeded_corruptions_are_reported_without_writing() {
    let dir = TempDir::new("inspect check");
    let baseline = summary(&check(&fixture("v1-typical.arc"), &Limits::DEFAULT).unwrap());
    let cases = [
        (
            "hash",
            "UPDATE arc_object SET payload_hash = zeroblob(32) WHERE id = ?1",
            vec![("doc.package.hash_mismatch", id_hex(1))],
        ),
        (
            "dangling",
            "DELETE FROM arc_object WHERE id = ?1",
            // Its `arc_property` row now names a missing object.
            vec![
                ("doc.reference.dangling", String::new()),
                ("doc.package.index_mismatch", id_hex(1)),
            ],
        ),
        (
            "property",
            "UPDATE arc_property SET value = 'other' WHERE object = ?1",
            vec![("doc.package.index_mismatch", id_hex(1))],
        ),
        (
            "reference",
            "DELETE FROM arc_reference WHERE source = ?1 AND field = 'end'",
            vec![("doc.package.index_mismatch", id_hex(3))],
        ),
    ];
    for (name, statement, expected) in cases {
        let path = dir.path(&format!("{name}.arc"));
        std::fs::copy(fixture("v1-typical.arc"), &path).unwrap();
        let target = if name == "reference" { 3 } else { 1 };
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            conn.execute(statement, [id_blob(target)]).unwrap(),
            1,
            "{name}"
        );
        drop(conn);
        let findings = untouched(&path, || check(&path, &Limits::DEFAULT).unwrap());
        let mut wanted = baseline.clone();
        for (code, id) in expected {
            wanted.push((code.to_owned(), id));
        }
        wanted.sort();
        let mut found = summary(&findings);
        if name == "dangling" {
            // The line names the deleted point in its `start` field.
            let dangling = findings
                .iter()
                .find(|f| f.code() == codes::REFERENCE_DANGLING.code)
                .unwrap();
            assert_eq!(
                dangling.arguments().get("field"),
                Some(&arc_base::error::Value::from("start"))
            );
            for entry in &mut found {
                if entry.0 == "doc.reference.dangling" {
                    entry.1 = String::new();
                }
            }
            found.sort();
        }
        assert_eq!(found, wanted, "{name}");
    }
}

#[test]
fn a_hot_journal_needs_recovery_before_inspection() {
    let dir = TempDir::new("inspect hot");
    let path = dir.path("hot.arc");
    std::fs::copy(fixture("v1-typical.arc"), &path).unwrap();
    // A write transaction whose changes reached the file, with its journal left beside it.
    let copy = dir.path("copy.arc");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA cache_size = 1; BEGIN;")
        .unwrap();
    conn.execute("UPDATE arc_object SET name = 'half'", [])
        .unwrap();
    conn.execute("DELETE FROM arc_meta", []).unwrap();
    std::fs::copy(&path, &copy).unwrap();
    let mut journal = path.as_os_str().to_owned();
    journal.push("-journal");
    let mut copy_journal = copy.as_os_str().to_owned();
    copy_journal.push("-journal");
    std::fs::copy(&journal, &copy_journal).unwrap();
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    for error in [
        untouched(&copy, || inspect(&copy, &Limits::DEFAULT).unwrap_err()),
        untouched(&copy, || check(&copy, &Limits::DEFAULT).unwrap_err()),
    ] {
        assert_eq!(error.code(), codes::PACKAGE_RECOVERY_NEEDED.code);
    }
    assert!(PathBuf::from(copy_journal).exists());
}
