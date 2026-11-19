//! Hostile ZIP input: each malformation is refused with its specific code before any entry is
//! inflated where the brief requires it, the bombs stop at their caps, and arbitrary or mutated
//! bytes never panic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::Limits;
use arc_io_common::ReadOptions;
use arc_io_common::zip::ZipArchive;
use proptest::prelude::*;
use support::{Layout, Spec, archive, archive_with, noise};

fn open_code(bytes: &[u8]) -> Option<&'static str> {
    ZipArchive::open(bytes, &ReadOptions::default())
        .err()
        .map(|e| e.code())
}

fn open_code_with(bytes: &[u8], limits: Limits) -> Option<&'static str> {
    ZipArchive::open(bytes, &ReadOptions::with_limits(limits))
        .err()
        .map(|e| e.code())
}

#[test]
fn unsafe_names_are_refused() {
    for name in ["../x", "/x", "a\\b", "C:x", "dir/../x", "a/./b"] {
        let bytes = archive(&[Spec::stored(name, b"x")]);
        assert_eq!(open_code(&bytes), Some("io.zip.path_unsafe"), "{name}");
    }
}

#[test]
fn malformed_names_are_refused() {
    let bytes = archive(&[Spec::stored("a//b", b"x")]);
    assert_eq!(open_code(&bytes), Some("io.zip.name_invalid"));
    let mut spec = Spec::stored("a", b"x");
    spec.name = b"a\0b".to_vec();
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.name_invalid"));
}

#[test]
fn duplicate_and_case_duplicate_names_are_refused() {
    let exact = archive(&[Spec::stored("a.txt", b"1"), Spec::stored("a.txt", b"2")]);
    assert_eq!(open_code(&exact), Some("io.zip.name_duplicate"));
    let folded = archive(&[
        Spec::stored("Docs/A.txt", b"1"),
        Spec::stored("docs/a.TXT", b"2"),
    ]);
    assert_eq!(open_code(&folded), Some("io.zip.name_duplicate"));
}

#[test]
fn encrypted_entries_are_refused() {
    let mut spec = Spec::stored("secret", b"x");
    spec.flags = 1;
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.encrypted"));
}

#[test]
fn unsupported_methods_are_refused() {
    let mut spec = Spec::stored("packed", b"x");
    spec.method = 12;
    assert_eq!(
        open_code(&archive(&[spec])),
        Some("io.zip.method_unsupported")
    );
}

#[test]
fn a_declared_size_bomb_is_refused_before_inflating() {
    let mut spec = Spec::deflated("bomb", b"tiny");
    spec.size = 10_000;
    assert_eq!(open_code(&archive(&[spec])), Some("io.limit_exceeded"));
}

#[test]
fn a_size_above_the_archive_budget_is_refused_before_inflating() {
    let mut spec = Spec::stored("huge", b"tiny");
    spec.size = 4 << 20;
    spec.compressed = 4 << 20;
    let limits = Limits {
        decompressed_bytes: 1 << 20,
        ..Limits::CONSTRAINED
    };
    assert_eq!(
        open_code_with(&archive(&[spec]), limits),
        Some("io.limit_exceeded")
    );
}

#[test]
fn the_total_declared_size_is_limited_over_entries() {
    let limits = Limits {
        decompressed_bytes: 1000,
        ..Limits::CONSTRAINED
    };
    let bytes = archive(&[
        Spec::stored("a", &noise(600, 1)),
        Spec::stored("b", &noise(600, 2)),
    ]);
    assert_eq!(open_code_with(&bytes, limits), Some("io.limit_exceeded"));
}

#[test]
fn a_ratio_bomb_stops_at_the_declared_cap_while_inflating() {
    // 4 MiB of zeros deflates to a few kilobytes; the declared size is small, so the stream
    // exceeds it and the reader stops one byte past the cap.
    let zeros = vec![0_u8; 4 << 20];
    let mut spec = Spec::deflated("zeros", &zeros);
    spec.size = 1000;
    let bytes = archive(&[spec]);
    let mut zip = ZipArchive::open(&bytes, &ReadOptions::default()).unwrap();
    let error = zip.read_entry(0).err().map(|e| e.code());
    assert_eq!(error, Some("io.zip.size_mismatch"));
}

#[test]
fn an_honest_ratio_above_the_limit_is_refused_before_inflating() {
    let zeros = vec![0_u8; 4 << 20];
    let bytes = archive(&[Spec::deflated("zeros", &zeros)]);
    assert_eq!(open_code(&bytes), Some("io.limit_exceeded"));
}

#[test]
fn a_stream_shorter_than_declared_is_a_size_mismatch() {
    let mut spec = Spec::deflated("short", &noise(100, 4));
    spec.size = 150;
    let bytes = archive(&[spec]);
    let mut zip = ZipArchive::open(&bytes, &ReadOptions::default()).unwrap();
    assert_eq!(
        zip.read_entry(0).err().map(|e| e.code()),
        Some("io.zip.size_mismatch")
    );
}

#[test]
fn too_many_entries_are_refused() {
    let limits = Limits {
        record_count: 1,
        ..Limits::CONSTRAINED
    };
    let bytes = archive(&[Spec::stored("a", b"1"), Spec::stored("b", b"2")]);
    assert_eq!(open_code_with(&bytes, limits), Some("io.limit_exceeded"));
}

#[test]
fn entries_overlapping_each_other_are_refused() {
    // The first entry declares 40 more bytes than it holds, so its data runs into the second
    // entry's local header, which exists in the buffer.
    let mut first = Spec::stored("first", &[b'x'; 10]);
    first.size += 40;
    first.compressed += 40;
    let second = Spec::stored("second", &noise(60, 5));
    let bytes = archive(&[first, second]);
    assert_eq!(open_code(&bytes), Some("io.zip.overlap"));
}

#[test]
fn an_entry_overlapping_the_central_directory_is_refused() {
    let mut spec = Spec::stored("runs-into-directory", &[b'x'; 10]);
    spec.size += 40;
    spec.compressed += 40;
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.overlap"));
}

#[test]
fn a_local_name_that_differs_is_a_header_mismatch() {
    let mut spec = Spec::stored("real.txt", b"x");
    spec.local_name = Some(b"other.txt".to_vec());
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.header_mismatch"));
}

#[test]
fn a_local_method_that_differs_is_a_header_mismatch() {
    let mut spec = Spec::deflated("d.txt", b"text text text");
    spec.local_method = Some(0);
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.header_mismatch"));
}

#[test]
fn a_local_crc_that_differs_is_a_header_mismatch() {
    let mut spec = Spec::stored("c.txt", b"x");
    spec.local_crc = Some(0);
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.header_mismatch"));
}

#[test]
fn a_data_descriptor_that_disagrees_is_unverified() {
    let mut spec = Spec::stored("desc.txt", b"data");
    spec.descriptor = Some((spec.crc ^ 1, spec.compressed, spec.size));
    assert_eq!(
        open_code(&archive(&[spec])),
        Some("io.zip.data_descriptor_unverified")
    );
}

#[test]
fn a_wrong_crc_is_refused_when_the_entry_is_read() {
    let mut spec = Spec::stored("bad-crc.txt", b"data");
    spec.crc ^= 0xFFFF;
    spec.local_crc = Some(spec.crc);
    let bytes = archive(&[spec]);
    let mut zip = ZipArchive::open(&bytes, &ReadOptions::default()).unwrap();
    assert_eq!(
        zip.read_entry(0).err().map(|e| e.code()),
        Some("io.zip.crc_mismatch")
    );
}

#[test]
fn a_corrupt_deflate_stream_is_refused() {
    let mut spec = Spec::deflated("garbage.bin", b"unused");
    spec.payload = vec![0xFF; 12];
    spec.compressed = 12;
    let bytes = archive(&[spec]);
    let mut zip = ZipArchive::open(&bytes, &ReadOptions::default()).unwrap();
    assert_eq!(
        zip.read_entry(0).err().map(|e| e.code()),
        Some("io.zip.inflate_failed")
    );
}

#[test]
fn a_multi_disk_archive_is_refused() {
    let layout = Layout {
        disk: 1,
        ..Layout::default()
    };
    let bytes = archive_with(&[Spec::stored("a", b"1")], &layout);
    assert_eq!(open_code(&bytes), Some("io.zip.multi_disk"));
}

#[test]
fn a_truncated_archive_is_refused() {
    let bytes = archive(&[Spec::stored("a", b"1234567890")]);
    assert_eq!(open_code(&bytes[..10]), Some("io.zip.end_missing"));
    assert_eq!(
        open_code(&bytes[..bytes.len() - 1]),
        Some("io.zip.end_missing")
    );
    assert_eq!(open_code(&[]), Some("io.zip.end_missing"));
}

#[test]
fn an_entry_whose_data_runs_past_the_buffer_is_truncated() {
    let mut spec = Spec::stored("cut", b"abc");
    spec.size += 100_000;
    spec.compressed += 100_000;
    assert_eq!(open_code(&archive(&[spec])), Some("io.zip.truncated"));
}

fn valid_archive() -> Vec<u8> {
    archive(&[
        Spec::stored("one.txt", b"alpha"),
        Spec::deflated("dir/two.txt", &b"beta beta beta".repeat(20)),
        Spec::stored("dir/", b""),
    ])
}

fn read_all(bytes: &[u8]) {
    let limits = Limits {
        input_bytes: 1 << 20,
        decompressed_bytes: 1 << 20,
        record_count: 1000,
        ..Limits::CONSTRAINED
    };
    let options = ReadOptions::with_limits(limits);
    if let Ok(mut zip) = ZipArchive::open(bytes, &options) {
        for index in 0..zip.entries().len() {
            if let Err(error) = zip.read_entry(index) {
                assert!(error.code().starts_with("io."), "{}", error.code());
            }
        }
    }
}

#[test]
fn a_valid_archive_reads_without_error() {
    let bytes = valid_archive();
    read_all(&bytes);
    assert_eq!(open_code(&bytes), None);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        read_all(&bytes);
    }

    #[test]
    fn mutations_of_a_valid_archive_never_panic(
        edits in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..8),
        truncate in any::<proptest::sample::Index>(),
    ) {
        let mut bytes = valid_archive();
        for (at, value) in edits {
            let position = at.index(bytes.len());
            if let Some(byte) = bytes.get_mut(position) {
                *byte = value;
            }
        }
        let keep = truncate.index(bytes.len() + 1);
        bytes.truncate(keep);
        read_all(&bytes);
    }
}

#[test]
fn a_zip64_size_beyond_the_limit_is_refused() {
    let layout = Layout {
        zip64: true,
        ..Layout::default()
    };
    let limits = Limits {
        decompressed_bytes: 100,
        ..Limits::CONSTRAINED
    };
    let bytes = archive_with(&[Spec::stored("big", &noise(200, 8))], &layout);
    assert_eq!(open_code_with(&bytes, limits), Some("io.limit_exceeded"));
}
