//! Reading well-formed archives: stored, deflated, directory, ZIP64 and data-descriptor entries
//! read back exactly, and the directory and budget rules of a valid archive hold.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::{Cancel, Limits};
use arc_io_common::ReadOptions;
use arc_io_common::zip::{Method, ZipArchive};
use support::{Layout, Spec, archive, archive_with, noise};

fn options() -> ReadOptions {
    ReadOptions::default()
}

#[test]
fn stored_and_deflated_entries_read_back() {
    let text = b"hello, archive\n".repeat(40);
    let binary = noise(5_000, 7);
    let bytes = archive(&[
        Spec::stored("plain.txt", b"hello"),
        Spec::deflated("text.txt", &text),
        Spec::deflated("bin/data.bin", &binary),
    ]);
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert_eq!(zip.entries().len(), 3);
    assert_eq!(zip.entries()[1].method, Method::Deflate);
    assert_eq!(zip.entries()[0].method, Method::Stored);
    assert_eq!(zip.read_entry(0).unwrap(), b"hello");
    assert_eq!(zip.read_entry(1).unwrap(), text);
    assert_eq!(zip.read_entry(2).unwrap(), binary);
}

#[test]
fn directory_entries_are_listed_and_read_empty() {
    let bytes = archive(&[Spec::stored("docs/", b""), Spec::stored("docs/a.txt", b"x")]);
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert!(zip.entries()[0].is_directory());
    assert!(!zip.entries()[1].is_directory());
    assert!(zip.read_entry(0).unwrap().is_empty());
}

#[test]
fn an_empty_archive_opens_with_no_entries() {
    let bytes = archive(&[]);
    let zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert!(zip.entries().is_empty());
}

#[test]
fn a_comment_after_the_end_record_is_allowed() {
    let layout = Layout {
        comment: b"archive comment".to_vec(),
        ..Layout::default()
    };
    let bytes = archive_with(&[Spec::stored("a", b"1")], &layout);
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert_eq!(zip.read_entry(0).unwrap(), b"1");
}

#[test]
fn zip64_records_within_the_limits_read_back() {
    let layout = Layout {
        zip64: true,
        ..Layout::default()
    };
    let bytes = archive_with(
        &[
            Spec::deflated("one.txt", b"first entry"),
            Spec::stored("two.txt", b"second"),
        ],
        &layout,
    );
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert_eq!(zip.entries().len(), 2);
    assert_eq!(zip.read_entry(0).unwrap(), b"first entry");
    assert_eq!(zip.read_entry(1).unwrap(), b"second");
}

#[test]
fn a_matching_data_descriptor_is_accepted() {
    let data = b"described data";
    let mut spec = Spec::stored("desc.txt", data);
    spec.descriptor = Some((spec.crc, spec.compressed, spec.size));
    let bytes = archive(&[spec]);
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    assert_eq!(zip.read_entry(0).unwrap(), data);
}

#[test]
fn reading_an_entry_twice_uses_the_budget_again() {
    let limits = Limits {
        decompressed_bytes: 1000,
        ..Limits::CONSTRAINED
    };
    let bytes = archive(&[Spec::deflated("a.bin", &noise(600, 3))]);
    let mut zip = ZipArchive::open(&bytes, &ReadOptions::with_limits(limits)).unwrap();
    zip.read_entry(0).unwrap();
    let error = zip.read_entry(0).err().map(|e| e.code());
    assert_eq!(error, Some("io.limit_exceeded"));
}

#[test]
fn a_cancelled_token_stops_the_inflate() {
    let cancel = Cancel::new();
    let options = ReadOptions {
        cancel: cancel.clone(),
        ..ReadOptions::default()
    };
    let bytes = archive(&[Spec::deflated("a.bin", &noise(100_000, 9))]);
    let mut zip = ZipArchive::open(&bytes, &options).unwrap();
    cancel.cancel();
    let error = zip.read_entry(0).err().map(|e| e.code());
    assert_eq!(error, Some("base.cancelled"));
}

#[test]
fn an_index_out_of_range_is_not_found() {
    let bytes = archive(&[Spec::stored("a", b"1")]);
    let mut zip = ZipArchive::open(&bytes, &options()).unwrap();
    let error = zip.read_entry(1).err().map(|e| e.code());
    assert_eq!(error, Some("io.zip.entry_missing"));
}
