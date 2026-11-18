//! Text encodings of DXF files (CORE-IO-010).
//!
//! Files before version 2007 store text in the legacy code page named by the header variable
//! `$DWGCODEPAGE`; version 2007 and later store UTF-8. In both, `\U+XXXX` escapes (four
//! hexadecimal digits, a UTF-16 code unit; surrogate pairs combine) decode to the character.
//!
//! Six single-byte code pages are implemented from the published Unicode mapping data: 1250
//! (Central European), 1251 (Cyrillic), 1252 (Western), 1253 (Greek), 1254 (Turkish) and 1257
//! (Baltic), named `ANSI_<number>` in the header. A byte that has no character in the code page,
//! and every byte of 0x80 or above under any other code page, decodes to the *raw-byte
//! character* U+F700 + byte, so encoding back to the same code page restores the byte exactly.
//!
//! The encoder writes a character the target cannot represent as a `\U+XXXX` escape (a pair for
//! characters beyond the Basic Multilingual Plane), and escapes line breaks, NUL and a backslash
//! that starts `U+`, so every string survives a write and a re-read.

/// The code-page names this crate implements, as they appear in `$DWGCODEPAGE`.
pub const CODE_PAGES: &[&str] = &[
    "ANSI_1250",
    "ANSI_1251",
    "ANSI_1252",
    "ANSI_1253",
    "ANSI_1254",
    "ANSI_1257",
];

/// First raw-byte character; byte `b` (0x80 or above) maps to `RAW_BASE + b`.
pub const RAW_BASE: u32 = 0xF700;

/// The upper half (0x80..=0xFF) of a single-byte code page; 0 marks a byte without character.
pub(crate) type Table = [u16; 128];

/// Characters 0x80..=0x9F shared by most Windows code pages, adjusted per page below.
const W80: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

// Indices stay below 256, so the casts are exact.
#[allow(clippy::cast_possible_truncation)]
const fn latin_upper() -> Table {
    let mut t = [0u16; 128];
    let mut i = 0;
    while i < 32 {
        t[i] = W80[i];
        i += 1;
    }
    while i < 128 {
        t[i] = 0x80 + i as u16;
        i += 1;
    }
    t
}

const fn patch(mut t: Table, changes: &[(u8, u16)]) -> Table {
    let mut i = 0;
    while i < changes.len() {
        t[(changes[i].0 - 0x80) as usize] = changes[i].1;
        i += 1;
    }
    t
}

const CP1252: Table = latin_upper();

const CP1254: Table = patch(
    latin_upper(),
    &[
        (0x8E, 0),
        (0x9E, 0),
        (0xD0, 0x011E),
        (0xDD, 0x0130),
        (0xDE, 0x015E),
        (0xF0, 0x011F),
        (0xFD, 0x0131),
        (0xFE, 0x015F),
    ],
);

const CP1250: Table = [
    0x20AC, 0, 0x201A, 0, 0x201E, 0x2026, 0x2020, 0x2021, 0, 0x2030, 0x0160, 0x2039, 0x015A,
    0x0164, 0x017D, 0x0179, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0, 0x2122,
    0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A, 0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104,
    0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B, 0x00B0, 0x00B1,
    0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD,
    0x013E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7, 0x010C, 0x00C9,
    0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150,
    0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF, 0x0155, 0x00E1,
    0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED,
    0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7, 0x0159, 0x016F,
    0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

// Indices stay below 256, so the casts are exact.
#[allow(clippy::cast_possible_truncation)]
const fn cyrillic() -> Table {
    let head: [u16; 64] = [
        0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409,
        0x2039, 0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E,
        0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC,
        0x00AD, 0x00AE, 0x0407, 0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
        0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    ];
    let mut t = [0u16; 128];
    let mut i = 0;
    while i < 64 {
        t[i] = head[i];
        t[64 + i] = 0x0410 + i as u16;
        i += 1;
    }
    t
}

const CP1251: Table = cyrillic();

// Indices stay below 256, so the casts are exact.
#[allow(clippy::cast_possible_truncation)]
const fn greek() -> Table {
    let mut t = latin_upper();
    let undefined: [u8; 15] = [
        0x81, 0x88, 0x8A, 0x8C, 0x8D, 0x8E, 0x8F, 0x90, 0x98, 0x9A, 0x9C, 0x9D, 0x9E, 0x9F, 0xAA,
    ];
    let mut i = 0;
    while i < undefined.len() {
        t[(undefined[i] - 0x80) as usize] = 0;
        i += 1;
    }
    t = patch(
        t,
        &[
            (0xA1, 0x0385),
            (0xA2, 0x0386),
            (0xAF, 0x2015),
            (0xB4, 0x0384),
            (0xB8, 0x0388),
            (0xB9, 0x0389),
            (0xBA, 0x038A),
            (0xBC, 0x038C),
            (0xBE, 0x038E),
            (0xBF, 0x038F),
            (0xD2, 0),
            (0xFF, 0),
        ],
    );
    let mut b = 0xC0;
    while b <= 0xFE {
        if b != 0xD2 {
            t[b - 0x80] = 0x0390 + (b - 0xC0) as u16;
        }
        b += 1;
    }
    t
}

const CP1253: Table = greek();

const CP1257: Table = [
    0x20AC, 0, 0x201A, 0, 0x201E, 0x2026, 0x2020, 0x2021, 0, 0x2030, 0, 0x2039, 0, 0x00A8, 0x02C7,
    0x00B8, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0, 0x2122, 0, 0x203A, 0,
    0x00AF, 0x02DB, 0, 0x00A0, 0, 0x00A2, 0x00A3, 0x00A4, 0, 0x00A6, 0x00A7, 0x00D8, 0x00A9,
    0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6, 0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5,
    0x00B6, 0x00B7, 0x00F8, 0x00B9, 0x0157, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00E6, 0x0104, 0x012E,
    0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112, 0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136,
    0x012A, 0x013B, 0x0160, 0x0143, 0x0145, 0x00D3, 0x014C, 0x00D5, 0x00D6, 0x00D7, 0x0172, 0x0141,
    0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF, 0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5,
    0x0119, 0x0113, 0x010D, 0x00E9, 0x017A, 0x0117, 0x0123, 0x0137, 0x012B, 0x013C, 0x0161, 0x0144,
    0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7, 0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C,
    0x017E, 0x02D9,
];

/// The table of a code-page name (case-insensitive), or `None` when it is not implemented.
pub(crate) fn table(name: &str) -> Option<&'static Table> {
    let upper = name.trim().to_ascii_uppercase();
    Some(match upper.as_str() {
        "ANSI_1250" => &CP1250,
        "ANSI_1251" => &CP1251,
        "ANSI_1252" => &CP1252,
        "ANSI_1253" => &CP1253,
        "ANSI_1254" => &CP1254,
        "ANSI_1257" => &CP1257,
        _ => return None,
    })
}

/// Whether a code page name is implemented.
#[must_use]
pub fn is_supported(code_page: &str) -> bool {
    table(code_page).is_some()
}

/// How text bytes of one file become characters.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Decoder {
    /// UTF-8 (version 2007 and later).
    Utf8,
    /// A single-byte legacy code page; `None` maps every byte of 0x80 and above to a raw-byte
    /// character.
    Legacy(Option<&'static Table>),
}

impl Decoder {
    pub(crate) fn legacy(code_page: &str) -> Self {
        Self::Legacy(table(code_page))
    }

    /// Decodes one text value; returns the text and the number of raw-byte characters produced,
    /// or `None` for invalid UTF-8.
    pub(crate) fn decode(self, bytes: &[u8]) -> Option<(String, u64)> {
        let mut raw = 0;
        let text = match self {
            Self::Utf8 => std::str::from_utf8(bytes).ok()?.to_owned(),
            Self::Legacy(table) => bytes
                .iter()
                .map(|&b| {
                    let mapped = match (b, table) {
                        (0..0x80, _) => u32::from(b),
                        (_, Some(t)) => t.get(usize::from(b - 0x80)).map_or(0, |&c| u32::from(c)),
                        (_, None) => 0,
                    };
                    if mapped == 0 && b != 0 {
                        raw += 1;
                        char::from_u32(RAW_BASE + u32::from(b)).unwrap_or('\u{FFFD}')
                    } else {
                        char::from_u32(mapped).unwrap_or('\u{FFFD}')
                    }
                })
                .collect(),
        };
        Some((decode_escapes(&text), raw))
    }
}

/// Decodes a text in a legacy code page into UTF-8, including `\U+XXXX` escapes.
#[must_use]
pub fn decode_legacy(bytes: &[u8], code_page: &str) -> String {
    Decoder::legacy(code_page)
        .decode(bytes)
        .map(|(text, _)| text)
        .unwrap_or_default()
}

/// Encodes a text into a legacy code page; characters outside it become `\U+XXXX` escapes and
/// raw-byte characters become their byte.
#[must_use]
pub fn encode_legacy(text: &str, code_page: &str) -> Vec<u8> {
    encode(text, Some(table(code_page)))
}

/// Encodes a text for a UTF-8 file, escaping only what the line and string structure needs.
#[must_use]
pub fn encode_utf8(text: &str) -> Vec<u8> {
    encode(text, None)
}

/// `legacy` is `None` for UTF-8 output and `Some(table)` for a legacy code page.
pub(crate) fn encode(text: &str, legacy: Option<Option<&'static Table>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let escape = match c {
            '\n' | '\r' | '\0' => true,
            '\\' => text
                .get(at + 1..)
                .is_some_and(|rest| rest.starts_with("U+")),
            _ => false,
        };
        if escape {
            push_escape(&mut out, c);
            continue;
        }
        match legacy {
            None => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            Some(table) => {
                let code = u32::from(c);
                if code < 0x80 {
                    out.extend(u8::try_from(code).ok());
                } else if let Some(byte) = legacy_byte(code, table) {
                    out.push(byte);
                } else {
                    push_escape(&mut out, c);
                }
            }
        }
    }
    out
}

fn legacy_byte(code: u32, table: Option<&'static Table>) -> Option<u8> {
    if let Some(t) = table
        && let Some(i) = t.iter().position(|&c| c != 0 && u32::from(c) == code)
    {
        return u8::try_from(i + 0x80).ok();
    }
    // A raw-byte character goes back to its byte when that byte has no character of its own.
    let byte = u8::try_from(code.checked_sub(RAW_BASE)?).ok()?;
    let free = match table {
        Some(t) => t.get(usize::from(byte.checked_sub(0x80)?)) == Some(&0),
        None => byte >= 0x80,
    };
    free.then_some(byte)
}

fn push_escape(out: &mut Vec<u8>, c: char) {
    let mut units = [0u16; 2];
    for unit in c.encode_utf16(&mut units) {
        out.extend_from_slice(format!("\\U+{unit:04X}").as_bytes());
    }
}

/// Replaces `\U+XXXX` escapes by their characters; a lone surrogate stays as written.
#[must_use]
pub fn decode_escapes(text: &str) -> String {
    if !text.contains("\\U+") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\\U+") {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at..).unwrap_or_default();
        match escape_unit(after) {
            Some(high @ 0xD800..=0xDBFF) => {
                let low = after.get(7..).and_then(escape_unit);
                match low {
                    Some(low @ 0xDC00..=0xDFFF) => {
                        let code = 0x10000
                            + ((u32::from(high) - 0xD800) << 10)
                            + (u32::from(low) - 0xDC00);
                        out.extend(char::from_u32(code));
                        rest = after.get(14..).unwrap_or_default();
                    }
                    _ => {
                        out.push_str(after.get(..7).unwrap_or_default());
                        rest = after.get(7..).unwrap_or_default();
                    }
                }
            }
            Some(unit) => match char::from_u32(u32::from(unit)) {
                Some(c) => {
                    out.push(c);
                    rest = after.get(7..).unwrap_or_default();
                }
                None => {
                    out.push_str(after.get(..7).unwrap_or_default());
                    rest = after.get(7..).unwrap_or_default();
                }
            },
            None => {
                out.push('\\');
                rest = after.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// The code unit of an escape at the start of `text`, if `text` starts with `\U+` and four
/// hexadecimal digits.
fn escape_unit(text: &str) -> Option<u16> {
    let digits = text.strip_prefix("\\U+")?.get(..4)?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_injective_and_ascii_free() {
        for name in CODE_PAGES {
            let t = table(name).unwrap();
            let mut seen = std::collections::BTreeSet::new();
            for &c in t.iter().filter(|&&c| c != 0) {
                assert!(c >= 0x80, "{name}");
                assert!(seen.insert(c), "{name}: {c:04X} twice");
            }
        }
    }

    #[test]
    fn turkish_letters_decode_from_1254() {
        let bytes = [
            0xD0, 0xDD, 0xDE, 0xF0, 0xFD, 0xFE, 0xC7, 0xE7, 0xD6, 0xF6, 0xDC, 0xFC,
        ];
        assert_eq!(decode_legacy(&bytes, "ANSI_1254"), "ĞİŞğışÇçÖöÜü");
        assert_eq!(encode_legacy("ĞİŞğışÇçÖöÜü", "ansi_1254"), bytes);
    }

    #[test]
    fn known_letters_of_each_page() {
        assert_eq!(decode_legacy(&[0xC0, 0xFF], "ANSI_1251"), "Ая");
        assert_eq!(decode_legacy(&[0xC1, 0xF9], "ANSI_1253"), "Αω");
        assert_eq!(decode_legacy(&[0xA3, 0xB9], "ANSI_1250"), "Łą");
        assert_eq!(decode_legacy(&[0xD0, 0xF0], "ANSI_1257"), "Šš");
        assert_eq!(decode_legacy(&[0x80, 0xE9], "ANSI_1252"), "€é");
    }

    #[test]
    fn every_byte_round_trips_through_every_page() {
        // Line breaks are escaped on purpose, so they are left out here.
        let bytes: Vec<u8> = (1..=255).filter(|b| *b != b'\n' && *b != b'\r').collect();
        for name in CODE_PAGES.iter().copied().chain(["ANSI_932"]) {
            let text = decode_legacy(&bytes, name);
            assert_eq!(encode_legacy(&text, name), bytes, "{name}");
        }
    }

    #[test]
    fn unrepresentable_characters_escape_and_come_back() {
        for text in [
            "日本 😀",
            "a\nb\r\0c",
            "literal \\U+0041 stays",
            "\\",
            "x\\U+",
        ] {
            for page in ["ANSI_1252", "ANSI_1254"] {
                let encoded = encode_legacy(text, page);
                assert!(encoded.iter().all(|&b| b != b'\n' && b != 0));
                assert_eq!(decode_legacy(&encoded, page), text, "{text:?} {page}");
            }
            let utf8 = encode_utf8(text);
            let back = Decoder::Utf8.decode(&utf8).unwrap().0;
            assert_eq!(back, text);
        }
        assert_eq!(encode_legacy("ğ", "ANSI_1252"), b"\\U+011F");
    }

    #[test]
    fn malformed_escapes_stay_literal() {
        assert_eq!(decode_escapes("\\U+12"), "\\U+12");
        assert_eq!(decode_escapes("\\U+D800x"), "\\U+D800x");
        assert_eq!(decode_escapes("\\U+D83D\\U+DE00"), "😀");
        assert_eq!(decode_escapes("a\\U+00e9"), "aé");
    }
}
