//! Strings of the physical file: the text between apostrophes, with the encoded forms of
//! ISO 10303-21 decoded to UTF-8.

use arc_base::Result;

use crate::error::{at_offset, codes};

/// The decoded text of a string and the leniency counts the reader reports.
#[derive(Debug, Default)]
pub(crate) struct Decoded {
    /// The text in UTF-8.
    pub(crate) text: String,
    /// Bytes that are not part of a UTF-8 sequence, read as ISO 8859-1.
    pub(crate) latin1: u64,
    /// Raw tab, carriage return and line feed bytes, kept as they are.
    pub(crate) controls: u64,
}

/// Decodes the raw content of a string (the bytes between its delimiters, with `''` still
/// doubled) that starts at `offset` in the file.
///
/// Supported forms: `''`, `\\`, `\S\c` (ISO 8859-1, the page in effect by default or after `\PA\`),
/// `\X\hh`, `\X2\...\X0\` (UTF-16 code units) and `\X4\...\X0\` (code points). Only `\S\` depends on
/// the ISO 8859 page; `\X\`, `\X2\` and `\X4\` always denote their Unicode or Latin-1 characters,
/// whatever page is selected. The `\P?\` letters A to I are accepted; the pages other than A have
/// no table, so `\S\` after one of them is refused rather than decoded wrongly. A decoded U+0000 is
/// refused as a raw NUL is. Raw bytes are UTF-8; a byte that starts no valid UTF-8 sequence is read
/// as ISO 8859-1 and counted.
pub(crate) fn decode(raw: &[u8], offset: u64) -> Result<Decoded> {
    let fail = || at_offset(codes::IFC_TEXT_INVALID, offset);
    let mut out = Decoded {
        text: String::with_capacity(raw.len()),
        latin1: 0,
        controls: 0,
    };
    let mut latin1_page = true;
    let mut i = 0usize;
    while let Some(&byte) = raw.get(i) {
        if byte == b'\'' && raw.get(i + 1) == Some(&b'\'') {
            out.text.push('\'');
            i += 2;
        } else if byte == b'\\' {
            let rest = raw.get(i + 1..).unwrap_or_default();
            match rest {
                [b'\\', ..] => {
                    out.text.push('\\');
                    i += 2;
                }
                [b'P', letter, b'\\', ..] if (b'A'..=b'I').contains(letter) => {
                    latin1_page = *letter == b'A';
                    i += 4;
                }
                [b'S', b'\\', c, ..] if (0x20..=0x7E).contains(c) => {
                    if !latin1_page {
                        return Err(fail());
                    }
                    out.text.push(char::from(*c | 0x80));
                    i += 4;
                }
                [b'X', b'\\', high, low, ..] => {
                    let value = hex_u32(&[*high, *low]).ok_or_else(fail)?;
                    let c = char::from(u8::try_from(value).map_err(|_| fail())?);
                    if c == '\0' {
                        return Err(fail());
                    }
                    out.text.push(c);
                    i += 5;
                }
                [b'X', width @ (b'2' | b'4'), b'\\', ..] => {
                    let digits = if *width == b'2' { 4 } else { 8 };
                    let (units, next) = collect_units(raw, i + 4, digits, &fail)?;
                    for decoded in char::decode_utf16(units) {
                        let c = decoded.map_err(|_| fail())?;
                        if c == '\0' {
                            return Err(fail());
                        }
                        out.text.push(c);
                    }
                    i = next;
                }
                _ => return Err(fail()),
            }
        } else if byte < 0x20 {
            // A raw tab, carriage return or line feed is kept; any other control byte, NUL included,
            // is refused.
            if !matches!(byte, b'\t' | b'\n' | b'\r') {
                return Err(fail());
            }
            out.text.push(char::from(byte));
            out.controls = out.controls.saturating_add(1);
            i += 1;
        } else if byte < 0x80 {
            out.text.push(char::from(byte));
            i += 1;
        } else if let Some(c) = utf8_char(raw.get(i..).unwrap_or_default()) {
            out.text.push(c);
            i += c.len_utf8();
        } else {
            out.text.push(char::from(byte));
            out.latin1 = out.latin1.saturating_add(1);
            i += 1;
        }
    }
    Ok(out)
}

/// The character at the start of `bytes`, when the bytes begin with one well-formed UTF-8 sequence.
fn utf8_char(bytes: &[u8]) -> Option<char> {
    let head = bytes.get(..bytes.len().min(4))?;
    let valid = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(head.get(..error.valid_up_to())?).ok()?,
    };
    valid.chars().next()
}

/// Reads groups of `digits` hexadecimal digits from `start` up to the terminator `\X0\`. Each
/// group is a UTF-16 code unit (`digits` 4) or a code point (`digits` 8); code points are encoded
/// to UTF-16 here, so the caller decodes one sequence. Returns the units and the index after the
/// terminator.
fn collect_units(
    raw: &[u8],
    start: usize,
    digits: usize,
    fail: &dyn Fn() -> arc_base::Error,
) -> Result<(Vec<u16>, usize)> {
    let mut units = Vec::new();
    let mut i = start;
    loop {
        if raw.get(i..i + 4) == Some(b"\\X0\\") {
            return Ok((units, i + 4));
        }
        let group = raw.get(i..i + digits).ok_or_else(fail)?;
        let value = hex_u32(group).ok_or_else(fail)?;
        if digits == 4 {
            units.push(u16::try_from(value).map_err(|_| fail())?);
        } else {
            let code = char::from_u32(value).ok_or_else(fail)?;
            let mut buffer = [0u16; 2];
            units.extend_from_slice(code.encode_utf16(&mut buffer));
        }
        i += digits;
    }
}

fn hex_u32(digits: &[u8]) -> Option<u32> {
    let mut value: u32 = 0;
    for &digit in digits {
        let nibble = char::from(digit).to_digit(16)?;
        value = value.checked_mul(16)?.checked_add(nibble)?;
    }
    Some(value)
}
