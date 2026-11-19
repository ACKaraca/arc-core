//! Property tests: no input makes `read` unwind, and a panic in the wrapped library is a reported
//! error, never a panic of the caller.

#![allow(clippy::panic, clippy::unwrap_used, reason = "tests")]

mod common;

use acadrust::types::DxfVersion;
use arc_base::Limits;
use arc_io_dwg::read;
use arc_io_dxf::ReadOptions;
use proptest::prelude::*;

const SIGNATURES: [&[u8; 6]; 8] = [
    b"AC1012", b"AC1014", b"AC1015", b"AC1018", b"AC1021", b"AC1024", b"AC1027", b"AC1032",
];

/// Limits that keep one read small: the library's decompression budget is 16 MiB.
fn small_limits() -> Limits {
    Limits {
        decompressed_bytes: 16 * 1024 * 1024,
        ..Limits::CONSTRAINED
    }
}

fn run(bytes: &[u8]) -> Result<(), String> {
    let mut options = ReadOptions::with_limits(small_limits());
    match read(bytes, &mut options) {
        Ok(_) => Ok(()),
        Err(err) if err.code() == "io.dwg.library_failed" => {
            Err(format!("the library panicked: {bytes:02x?}"))
        }
        Err(_) => Ok(()),
    }
}

fn valid_sample() -> Vec<u8> {
    common::dwg_bytes(&common::sample_document(DxfVersion::AC1032))
}

proptest! {
    #![proptest_config(ProptestConfig::default())]

    #[test]
    fn arbitrary_bytes_never_unwind(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        prop_assert!(run(&bytes).is_ok());
    }

    #[test]
    fn supported_signatures_with_random_tails_never_unwind(
        index in 0usize..8,
        tail in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        let mut bytes = SIGNATURES[index].to_vec();
        bytes.extend_from_slice(&tail);
        prop_assert!(run(&bytes).is_ok());
    }

    #[test]
    fn mutations_of_a_valid_file_never_unwind(
        edits in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..=128),
    ) {
        // Positions are drawn over the whole file, not only its head.
        let mut bytes = valid_sample();
        for (position, value) in edits {
            let at = position.index(bytes.len());
            if let Some(byte) = bytes.get_mut(at) {
                *byte = value;
            }
        }
        prop_assert!(run(&bytes).is_ok());
    }
}
