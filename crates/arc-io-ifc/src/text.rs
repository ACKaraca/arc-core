//! Strings of the physical file: the text between apostrophes, with the encoded forms of
//! ISO 10303-21 decoded to UTF-8.

use arc_base::{Error, Result};

use crate::error::{at_offset, codes};

/// Decodes the raw content of a string (the bytes between its delimiters, with `''` still
/// doubled) that starts at `offset` in the file.
///
/// Supported forms: `''`, `\\`, `\S\c` and `\X\hh` (ISO 8859-1, the page in effect by default or
/// after `\PA\`), `\P?\` for the page letters A to I, `\X2\...\X0\` (UTF-16 code units) and
/// `\X4\...\X0\` (code points). The other ISO 8859 pages have no table here, so a string that
/// selects one is refused rather than decoded wrongly.
pub(crate) fn decode(raw: &[u8], offset: u64) -> Result<String> {
    let fail = || at_offset(codes::IFC_TEXT_INVALID, offset);
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut latin1 = true;
    let mut i = 0usize;
    while let Some(&byte) = raw.get(i) {
        if byte < 0x20 {
            return Err(fail());
        }
        if byte == b'\'' && raw.get(i + 1) == Some(&b'\'') {
            out.push(b'\'');
            i += 2;
        } else if byte != b'\\' {
            out.push(byte);
            i += 1;
        } else {
            let rest = raw.get(i + 1..).unwrap_or_default();
            match rest {
                [b'\\', ..] => {
                    out.push(b'\\');
                    i += 2;
                }
                [b'P', letter, b'\\', ..] if (b'A'..=b'I').contains(letter) => {
                    latin1 = *letter == b'A';
                    i += 4;
                }
                [b'S', b'\\', c, ..] if (0x20..=0x7E).contains(c) => {
                    if !latin1 {
                        return Err(fail());
                    }
                    push_char(&mut out, char::from(*c | 0x80));
                    i += 4;
                }
                [b'X', b'\\', high, low, ..] => {
                    if !latin1 {
                        return Err(fail());
                    }
                    let value = hex_u32(&[*high, *low]).ok_or_else(fail)?;
                    push_char(
                        &mut out,
                        char::from(u8::try_from(value).map_err(|_| fail())?),
                    );
                    i += 6;
                }
                [b'X', width @ (b'2' | b'4'), b'\\', ..] => {
                    let digits = if *width == b'2' { 4 } else { 8 };
                    let (units, next) = collect_units(raw, i + 4, digits, &fail)?;
                    for decoded in char::decode_utf16(units) {
                        push_char(&mut out, decoded.map_err(|_| fail())?);
                    }
                    i = next;
                }
                _ => return Err(fail()),
            }
        }
    }
    String::from_utf8(out).map_err(|_| fail())
}

/// Reads groups of `digits` hexadecimal digits from `start` up to the terminator `\X0\`. Each
/// group is a UTF-16 code unit (`digits` 4) or a code point (`digits` 8); code points are encoded
/// to UTF-16 here, so the caller decodes one sequence. Returns the units and the index after the
/// terminator.
fn collect_units(
    raw: &[u8],
    start: usize,
    digits: usize,
    fail: &dyn Fn() -> Error,
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

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buffer = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
}

fn hex_u32(digits: &[u8]) -> Option<u32> {
    let mut value: u32 = 0;
    for &digit in digits {
        let nibble = char::from(digit).to_digit(16)?;
        value = value.checked_mul(16)?.checked_add(nibble)?;
    }
    Some(value)
}
