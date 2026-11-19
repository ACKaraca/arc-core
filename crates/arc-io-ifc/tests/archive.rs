//! IFC-ZIP: the same file inside an archive reads identically; archives without or with several
//! `.ifc` entries are refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

#[path = "support/mod.rs"]
mod support;

use arc_io_ifc::{ReadOptions, read};

#[test]
fn ifc_zip_reads_like_the_physical_file() {
    let text = support::project("IFC4");
    let plain = read(text.as_bytes(), &mut ReadOptions::default()).unwrap();
    let archive = support::zip(&[
        ("readme.txt", b"not the model"),
        ("models/", b""),
        ("models/project.IFC", text.as_bytes()),
    ]);
    let zipped = read(&archive, &mut ReadOptions::default()).unwrap();
    assert_eq!(zipped.file, plain.file);
    assert_eq!(zipped.report, plain.report);
}

#[test]
fn archive_without_an_ifc_entry_is_refused() {
    let archive = support::zip(&[("readme.txt", b"nothing here")]);
    let error = read(&archive, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.ifc.archive_entry_missing");
}

#[test]
fn archive_with_two_ifc_entries_is_refused() {
    let text = support::project("IFC2X3");
    let archive = support::zip(&[("a.ifc", text.as_bytes()), ("b.ifc", text.as_bytes())]);
    let error = read(&archive, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.ifc.archive_entry_ambiguous");
}

#[test]
fn empty_archive_is_refused_as_having_no_ifc_entry() {
    let archive = support::zip(&[]);
    let error = read(&archive, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.ifc.archive_entry_missing");
}
