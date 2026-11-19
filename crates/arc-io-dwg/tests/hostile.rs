//! Errors of the input checks, the limits and cancellation, each with its specific code.

#![allow(clippy::panic, clippy::unwrap_used, reason = "tests")]

mod common;

use acadrust::types::DxfVersion;
use arc_base::{Cancel, Error, Limits};
use arc_io_dwg::read;
use arc_io_dxf::ReadOptions;

fn sample() -> Vec<u8> {
    common::dwg_bytes(&common::sample_document(DxfVersion::AC1032))
}

fn code_of(bytes: &[u8], options: &mut ReadOptions<'_>) -> Option<&'static str> {
    read(bytes, options).err().map(|e| e.code())
}

fn error_of(bytes: &[u8], options: &mut ReadOptions<'_>) -> Error {
    match read(bytes, options) {
        Ok(_) => panic!("the read was expected to fail"),
        Err(err) => err,
    }
}

#[test]
fn wrong_signature_is_rejected_before_the_library() {
    let mut bytes = sample();
    bytes[..6].copy_from_slice(b"ZZ1032");
    assert_eq!(
        code_of(&bytes, &mut ReadOptions::default()),
        Some("io.dwg.signature_invalid")
    );
    assert_eq!(
        code_of(b"AC10", &mut ReadOptions::default()),
        Some("io.dwg.signature_invalid")
    );
}

#[test]
fn unsupported_versions_are_rejected_with_their_signature() {
    for signature in [b"AC1009", b"AC1006", b"AC1040"] {
        let mut bytes = sample();
        bytes[..6].copy_from_slice(signature);
        let err = error_of(&bytes, &mut ReadOptions::default());
        assert_eq!(err.code(), "io.dwg.version_unsupported");
        assert_eq!(
            err.arguments().get("version").map(|v| format!("{v:?}")),
            Some(format!("Text({:?})", String::from_utf8_lossy(signature)))
        );
    }
}

#[test]
fn truncated_input_is_reported_as_truncated() {
    let bytes = sample();
    let half = &bytes[..bytes.len() / 2];
    assert_eq!(
        code_of(half, &mut ReadOptions::default()),
        Some("io.dwg.truncated")
    );
}

#[test]
fn input_over_the_size_limit_is_refused() {
    let bytes = sample();
    let limits = Limits {
        input_bytes: 1000,
        ..Limits::CONSTRAINED
    };
    let err = error_of(&bytes, &mut ReadOptions::with_limits(limits));
    assert_eq!(err.code(), "io.limit_exceeded");
    assert!(
        err.arguments()
            .get("limit")
            .is_some_and(|v| format!("{v:?}").contains("input_bytes"))
    );
}

#[test]
fn record_count_limit_is_enforced() {
    let bytes = sample();
    let limits = Limits {
        record_count: 2,
        ..Limits::CONSTRAINED
    };
    let err = error_of(&bytes, &mut ReadOptions::with_limits(limits));
    assert_eq!(err.code(), "io.limit_exceeded");
    assert!(format!("{:?}", err.arguments()).contains("record_count"));
}

#[test]
fn array_length_limit_is_enforced_on_vertices() {
    let bytes = sample();
    let limits = Limits {
        array_length: 2,
        ..Limits::CONSTRAINED
    };
    let err = error_of(&bytes, &mut ReadOptions::with_limits(limits));
    assert_eq!(err.code(), "io.limit_exceeded");
    assert!(format!("{:?}", err.arguments()).contains("array_length"));
}

#[test]
fn string_length_limit_is_enforced() {
    let bytes = sample();
    let limits = Limits {
        string_bytes: 2,
        ..Limits::CONSTRAINED
    };
    let err = error_of(&bytes, &mut ReadOptions::with_limits(limits));
    assert_eq!(err.code(), "io.limit_exceeded");
    assert!(format!("{:?}", err.arguments()).contains("string_bytes"));
}

#[test]
fn decompression_budget_is_passed_to_the_library() {
    let bytes = sample();
    let limits = Limits {
        decompressed_bytes: 16,
        ..Limits::CONSTRAINED
    };
    let err = error_of(&bytes, &mut ReadOptions::with_limits(limits));
    assert_eq!(err.code(), "io.limit_exceeded");
    assert!(format!("{:?}", err.arguments()).contains("decompressed_bytes"));
}

#[test]
fn cancellation_before_the_call_returns_cancelled() {
    let bytes = sample();
    let cancel = Cancel::new();
    cancel.cancel();
    let mut options = ReadOptions {
        cancel,
        ..ReadOptions::default()
    };
    assert_eq!(code_of(&bytes, &mut options), Some("base.cancelled"));
}

#[test]
fn empty_input_is_a_signature_error() {
    assert_eq!(
        code_of(&[], &mut ReadOptions::default()),
        Some("io.dwg.signature_invalid")
    );
}
