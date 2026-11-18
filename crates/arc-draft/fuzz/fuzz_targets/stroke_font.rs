//! Reads arbitrary bytes as a stroke font with two limits profiles; a font that reads must
//! stay inside its glyph boxes and read back equal from its canonical text.

#![no_main]

use arc_base::Limits;
use arc_draft::stroke_font::StrokeFont;
use libfuzzer_sys::fuzz_target;

/// Small enough for the fuzzer to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 256,
    array_length: 3,
    ..Limits::STRICT
};

fuzz_target!(|data: &[u8]| {
    for limits in [&Limits::STRICT, &TIGHT] {
        match StrokeFont::parse(data, limits) {
            Ok(font) => {
                assert!(font.check_bounds().is_ok());
                let text = font.to_text();
                let again = StrokeFont::parse(text.as_bytes(), &Limits::STRICT);
                assert_eq!(again.as_ref().ok(), Some(&font), "{text}");
            }
            Err(error) => assert!(error.code().starts_with("draft.font."), "{error:?}"),
        }
    }
});
