//! Decoding of character data: the five predefined entities, character references, line-end
//! normalization and the character set of XML.

use arc_base::{Error, Result};

use crate::error::codes;

/// What the text is: element content keeps line breaks, an attribute value turns whitespace into
/// spaces, as XML requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Text,
    Attribute,
}

/// Decodes the raw bytes of one text node or attribute value.
pub(super) fn decode(raw: &[u8], mode: Mode) -> Result<String> {
    let text = std::str::from_utf8(raw).map_err(|_| Error::new(codes::XML_TEXT_INVALID))?;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['&', '\r', '\n', '\t']) {
        let Some((plain, tail)) = rest.split_at_checked(at) else {
            break;
        };
        push_plain(&mut out, plain)?;
        if tail.starts_with('&') {
            let (ch, remaining) = reference(tail)?;
            out.push(ch);
            rest = remaining;
        } else if let Some(after) = tail.strip_prefix("\r\n") {
            out.push(line_end(mode, '\n'));
            rest = after;
        } else {
            let after = tail.get(1..).unwrap_or_default();
            out.push(match tail.chars().next() {
                Some('\r') => line_end(mode, '\n'),
                Some('\n') => line_end(mode, '\n'),
                Some(ch) => whitespace(mode, ch),
                None => ' ',
            });
            rest = after;
        }
    }
    push_plain(&mut out, rest)?;
    Ok(out)
}

/// The character a line end becomes: a line feed in content, a space in an attribute value.
fn line_end(mode: Mode, ch: char) -> char {
    whitespace(mode, ch)
}

fn whitespace(mode: Mode, ch: char) -> char {
    match mode {
        Mode::Text => ch,
        Mode::Attribute => ' ',
    }
}

fn push_plain(out: &mut String, plain: &str) -> Result<()> {
    if plain.chars().all(is_xml_char) {
        out.push_str(plain);
        Ok(())
    } else {
        Err(Error::new(codes::XML_TEXT_INVALID))
    }
}

/// Reads the reference at the start of `tail` and returns its character and the text after it.
fn reference(tail: &str) -> Result<(char, &str)> {
    let invalid = || Error::new(codes::XML_REFERENCE_INVALID);
    let body = tail.strip_prefix('&').ok_or_else(invalid)?;
    let end = body.find(';').ok_or_else(invalid)?;
    let name = body.get(..end).ok_or_else(invalid)?;
    let remaining = body.get(end + 1..).ok_or_else(invalid)?;
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ if name.starts_with('#') => character_reference(name)?,
        _ => return Err(Error::new(codes::XML_ENTITY_REFUSED)),
    };
    Ok((ch, remaining))
}

/// The character of a numeric reference such as `#x41` or `#65`.
fn character_reference(name: &str) -> Result<char> {
    let invalid = || Error::new(codes::XML_REFERENCE_INVALID);
    let (digits, radix) = match name.strip_prefix("#x") {
        Some(hex) => (hex, 16),
        None => (name.strip_prefix('#').ok_or_else(invalid)?, 10),
    };
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_digit(radix)) {
        return Err(invalid());
    }
    let value = u32::from_str_radix(digits, radix).map_err(|_| invalid())?;
    char::from_u32(value)
        .filter(|ch| is_xml_char(*ch))
        .ok_or_else(invalid)
}

/// Whether `ch` is a character of XML 1.0 (`Char` in the grammar).
pub(super) fn is_xml_char(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

#[cfg(test)]
mod tests {
    use arc_base::Error;

    use super::{Mode, decode};
    use crate::error::codes;

    fn code(raw: &[u8], mode: Mode) -> Option<&'static str> {
        decode(raw, mode).err().map(|error: Error| error.code())
    }

    #[test]
    fn predefined_entities_decode() {
        assert_eq!(
            decode(b"a&amp;&lt;&gt;&quot;&apos;b", Mode::Text).ok(),
            Some("a&<>\"'b".to_owned())
        );
    }

    #[test]
    fn numeric_references_decode_in_decimal_and_hex() {
        assert_eq!(
            decode(b"&#65;&#x42;&#x1F600;", Mode::Text).ok(),
            Some("AB\u{1F600}".to_owned())
        );
    }

    #[test]
    fn invalid_references_are_refused() {
        for raw in [
            &b"&#0;"[..],
            b"&#xD800;",
            b"&#x110000;",
            b"&#x;",
            b"&#+41;",
            b"&#xG;",
            b"&amp",
        ] {
            assert_eq!(
                code(raw, Mode::Text),
                Some(codes::XML_REFERENCE_INVALID.code),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn other_entities_are_refused() {
        assert_eq!(
            code(b"&custom;", Mode::Text),
            Some(codes::XML_ENTITY_REFUSED.code)
        );
    }

    #[test]
    fn line_ends_are_normalized_in_content() {
        assert_eq!(
            decode(b"a\r\nb\rc\n", Mode::Text).ok(),
            Some("a\nb\nc\n".to_owned())
        );
    }

    #[test]
    fn attribute_whitespace_becomes_spaces_but_references_do_not() {
        assert_eq!(
            decode(b"a\r\n\tb&#10;", Mode::Attribute).ok(),
            Some("a  b\n".to_owned())
        );
    }

    #[test]
    fn characters_outside_xml_are_refused() {
        assert_eq!(
            code(b"a\x01b", Mode::Text),
            Some(codes::XML_TEXT_INVALID.code)
        );
        assert_eq!(
            code(&[0xFF], Mode::Text),
            Some(codes::XML_TEXT_INVALID.code)
        );
    }
}
