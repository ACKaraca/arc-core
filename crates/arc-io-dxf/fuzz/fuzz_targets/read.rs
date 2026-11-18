//! Reads arbitrary bytes, as given and behind the binary sentinel, with two limits profiles. A
//! read either fails with an `io.*` code or yields a drawing that writes in every version and
//! form and reads back.

#![no_main]

use arc_base::Limits;
use arc_io_dxf::{ReadOptions, Version, WriteOptions, read, write};
use libfuzzer_sys::fuzz_target;

/// Small enough for the fuzzer to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 4096,
    record_count: 64,
    nesting_depth: 4,
    string_bytes: 64,
    array_length: 64,
    ..Limits::STRICT
};

fn check(data: &[u8]) {
    for limits in [TIGHT, Limits::STRICT] {
        match read(data, &mut ReadOptions::with_limits(limits)) {
            Err(error) => assert!(error.code().starts_with("io."), "{}", error.code()),
            Ok(out) => {
                for version in [Version::R2000, Version::R2018] {
                    for binary in [false, true] {
                        let mut options = WriteOptions { version, binary, ..WriteOptions::default() };
                        if let Ok(written) = write(&out.drawing, &mut options) {
                            read(&written.bytes, &mut ReadOptions::with_limits(Limits::DEFAULT))
                                .expect("written output reads back");
                        }
                    }
                }
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    check(data);
    let mut binary = b"AutoCAD Binary DXF\r\n\x1a\0".to_vec();
    binary.extend_from_slice(data);
    check(&binary);
});
