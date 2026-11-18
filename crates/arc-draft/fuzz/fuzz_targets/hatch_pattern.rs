//! Reads arbitrary bytes as a hatch-pattern file with two limits profiles; a file that reads
//! has valid patterns and reads back equal from its canonical text.

#![no_main]

use arc_base::Limits;
use arc_draft::hatch_pattern::PatternFile;
use libfuzzer_sys::fuzz_target;

/// Small enough for the fuzzer to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 256,
    array_length: 3,
    ..Limits::STRICT
};

fuzz_target!(|data: &[u8]| {
    for limits in [&Limits::STRICT, &TIGHT] {
        match PatternFile::parse(data, limits) {
            Ok(file) => {
                assert!(file.patterns.iter().all(|p| p.validate().is_ok()));
                let text = file.to_text();
                let again = PatternFile::parse(text.as_bytes(), &Limits::STRICT);
                assert_eq!(again.as_ref().ok(), Some(&file), "{text}");
            }
            Err(error) => assert!(error.code().starts_with("draft.pattern."), "{error:?}"),
        }
    }
});
