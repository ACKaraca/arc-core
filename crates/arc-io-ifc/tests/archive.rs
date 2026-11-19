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

use arc_base::Limits;
use arc_base::error::Value;
use arc_io_ifc::{ReadOptions, read, reasons};
use support::{Entry, deflated_run, zip_entries};

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

#[test]
fn deflated_entry_larger_than_input_bytes_is_refused_by_the_archive_limit() {
    // 2065 bytes once inflated from about 20 bytes packed: over the 512-byte input bound.
    let (packed, original) = deflated_run(8);
    let archive = zip_entries(&[Entry {
        name: "model.ifc",
        method: 8,
        payload: &packed,
        original: &original,
    }]);
    let limits = Limits {
        input_bytes: 512,
        ..Limits::CONSTRAINED
    };
    let error = read(&archive, &mut ReadOptions::with_limits(limits)).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("decompressed_bytes".to_owned()))
    );
}

#[test]
fn metadata_entries_are_skipped_when_choosing_the_model() {
    let text = support::project("IFC4");
    let plain = read(text.as_bytes(), &mut ReadOptions::default()).unwrap();
    let archive = support::zip(&[
        ("__MACOSX/._model.ifc", b"\x00\x05\x16 resource fork"),
        ("model.ifc", text.as_bytes()),
        ("._model.ifc", b"\x00\x05\x16 resource fork"),
    ]);
    let zipped = read(&archive, &mut ReadOptions::default()).unwrap();
    assert_eq!(zipped.file, plain.file);
    let skipped = zipped
        .report
        .items
        .iter()
        .find(|i| i.reason == reasons::ARCHIVE_ENTRY_SKIPPED.code)
        .expect("skipped entries reported");
    assert_eq!(
        skipped.arguments.get("count"),
        Some(&arc_io_ifc::loss::Argument::Int(2))
    );
}
