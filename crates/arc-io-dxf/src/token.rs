//! Splitting ASCII and binary DXF bytes into typed groups.

use arc_base::{Limits, Result};

use crate::encoding::Decoder;
use crate::error::{Position, codes, limit};
use crate::group::{Group, Value, ValueKind, value_kind};

/// The first bytes of a binary file.
pub(crate) const SENTINEL: &[u8] = b"AutoCAD Binary DXF\r\n\x1a\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Ascii,
    /// `short_codes`: one-byte group codes with 255 as escape (version R12 binary files).
    Binary {
        short_codes: bool,
    },
}

impl Format {
    /// Detects the format and the offset where groups start.
    pub(crate) fn detect(data: &[u8]) -> (Self, usize) {
        if let Some(rest) = data.strip_prefix(SENTINEL) {
            // The first group is `0 SECTION` or a comment (999): two-byte codes start with
            // `00 00` or `E7 03`, one-byte codes with `00 'S'` or `FF E7 03`.
            let short_codes = !matches!(rest.get(..2), Some([0, 0] | [0xE7, 0x03]));
            return (Self::Binary { short_codes }, SENTINEL.len());
        }
        let bom = if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        (Self::Ascii, bom)
    }
}

pub(crate) struct Tokens<'a> {
    data: &'a [u8],
    pos: usize,
    line: u64,
    format: Format,
    decoder: Decoder,
    limits: &'a Limits,
    /// Raw-byte characters produced while decoding text.
    pub(crate) raw_bytes: u64,
    /// Comment groups skipped.
    pub(crate) comments: u64,
}

impl<'a> Tokens<'a> {
    pub(crate) fn new(data: &'a [u8], decoder: Decoder, limits: &'a Limits) -> Self {
        let (format, pos) = Format::detect(data);
        Self {
            data,
            pos,
            line: 0,
            format,
            decoder,
            limits,
            raw_bytes: 0,
            comments: 0,
        }
    }

    pub(crate) fn offset(&self) -> usize {
        self.pos
    }

    /// The next group that is not a comment, with its position.
    pub(crate) fn next(&mut self) -> Result<Option<(Group, Position)>> {
        loop {
            let next = match self.format {
                Format::Ascii => self.next_ascii()?,
                Format::Binary { short_codes } => self.next_binary(short_codes)?,
            };
            match next {
                Some((g, _)) if g.code == 999 => self.comments += 1,
                other => return Ok(other),
            }
        }
    }

    fn line(&mut self) -> Option<&'a [u8]> {
        let rest = self.data.get(self.pos..)?;
        if rest.is_empty() {
            return None;
        }
        let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        let line = rest.get(..end).unwrap_or_default();
        self.pos = self.pos.saturating_add(end + 1).min(self.data.len());
        self.line += 1;
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }

    fn next_ascii(&mut self) -> Result<Option<(Group, Position)>> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        if rest.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        let Some(code_line) = self.line() else {
            return Ok(None);
        };
        let at = Position::Line(self.line);
        let code = std::str::from_utf8(code_line)
            .ok()
            .and_then(|s| s.trim().parse::<i16>().ok())
            .filter(|&c| c >= 0)
            .ok_or_else(|| at.error(codes::DXF_GROUP_CODE_INVALID))?;
        let value_line = self.line().ok_or_else(|| at.error(codes::DXF_TRUNCATED))?;
        let kind = value_kind(code).unwrap_or(ValueKind::Text);
        let invalid = || at.error(codes::DXF_VALUE_INVALID);
        let number = || {
            std::str::from_utf8(value_line)
                .ok()
                .map(str::trim)
                .ok_or_else(invalid)
        };
        let value = match kind {
            ValueKind::Text => Value::Text(self.text(value_line, at)?),
            ValueKind::Double => {
                let v: f64 = number()?.parse().map_err(|_| invalid())?;
                if !v.is_finite() {
                    return Err(invalid());
                }
                Value::Double(v)
            }
            ValueKind::Bool => Value::Bool(number()?.parse::<i64>().map_err(|_| invalid())? != 0),
            ValueKind::Bytes => {
                let hex = number()?;
                self.check_string(hex.len() / 2)?;
                Value::Bytes(parse_hex(hex).ok_or_else(invalid)?)
            }
            int => {
                let v: i64 = number()?.parse().map_err(|_| invalid())?;
                Value::Int(int_in_range(int, v).ok_or_else(invalid)?)
            }
        };
        Ok(Some((Group { code, value }, at)))
    }

    fn check_string(&self, len: usize) -> Result<()> {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        if len > self.limits.string_bytes {
            return Err(limit("string_bytes", len));
        }
        Ok(())
    }

    fn text(&mut self, bytes: &[u8], at: Position) -> Result<String> {
        self.check_string(bytes.len())?;
        let (text, raw) = self
            .decoder
            .decode(bytes)
            .ok_or_else(|| at.error(codes::DXF_TEXT_INVALID))?;
        self.raw_bytes += raw;
        Ok(text)
    }

    fn take(&mut self, n: usize, at: Position) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| at.error(codes::DXF_TRUNCATED))?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| at.error(codes::DXF_TRUNCATED))?;
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self, at: Position) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, at)?);
        Ok(out)
    }

    fn next_binary(&mut self, short_codes: bool) -> Result<Option<(Group, Position)>> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let at = Position::Offset(u64::try_from(self.pos).unwrap_or(u64::MAX));
        let code = if short_codes {
            match self.array::<1>(at)? {
                [255] => i16::from_le_bytes(self.array(at)?),
                [b] => i16::from(b),
            }
        } else {
            i16::from_le_bytes(self.array(at)?)
        };
        let kind = value_kind(code).ok_or_else(|| at.error(codes::DXF_GROUP_CODE_INVALID))?;
        let value = match kind {
            ValueKind::Text => {
                let rest = self.data.get(self.pos..).unwrap_or_default();
                let window = usize::try_from(self.limits.string_bytes)
                    .unwrap_or(usize::MAX)
                    .saturating_add(1);
                let Some(end) = rest.iter().take(window).position(|&b| b == 0) else {
                    return Err(if rest.len() >= window {
                        limit("string_bytes", u64::try_from(window).unwrap_or(u64::MAX))
                    } else {
                        at.error(codes::DXF_TRUNCATED)
                    });
                };
                let bytes = self.take(end + 1, at)?;
                Value::Text(self.text(bytes.get(..end).unwrap_or_default(), at)?)
            }
            ValueKind::Double => {
                let v = f64::from_le_bytes(self.array(at)?);
                if !v.is_finite() {
                    return Err(at.error(codes::DXF_VALUE_INVALID));
                }
                Value::Double(v)
            }
            ValueKind::Int8 => Value::Int(i64::from(u8::from_le_bytes(self.array(at)?))),
            ValueKind::Int16 => Value::Int(i64::from(i16::from_le_bytes(self.array(at)?))),
            ValueKind::Int32 => Value::Int(i64::from(i32::from_le_bytes(self.array(at)?))),
            ValueKind::Int64 => Value::Int(i64::from_le_bytes(self.array(at)?)),
            ValueKind::Bool => Value::Bool(self.array::<1>(at)? != [0]),
            ValueKind::Bytes => {
                let [n] = self.array::<1>(at)?;
                Value::Bytes(self.take(usize::from(n), at)?.to_vec())
            }
        };
        Ok(Some((Group { code, value }, at)))
    }
}

/// The value as stored for an integer kind, or `None` when it does not fit. Negative 8-bit
/// values are stored as their unsigned byte.
pub(crate) fn int_in_range(kind: ValueKind, v: i64) -> Option<i64> {
    match kind {
        ValueKind::Int8 => match v {
            -128..=-1 => Some(v + 256),
            0..=255 => Some(v),
            _ => None,
        },
        ValueKind::Int16 => i16::try_from(v).ok().map(i64::from),
        ValueKind::Int32 => i32::try_from(v).ok().map(i64::from),
        _ => Some(v),
    }
}

fn parse_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(data: &[u8]) -> Result<Vec<Group>> {
        let limits = Limits::STRICT;
        let mut t = Tokens::new(data, Decoder::Utf8, &limits);
        let mut out = Vec::new();
        while let Some((g, _)) = t.next()? {
            out.push(g);
        }
        Ok(out)
    }

    #[test]
    fn ascii_groups_are_typed_by_code() {
        let gs = groups(b"  0\r\nLINE\r\n 10\r\n1.5\r\n 70\r\n-3\r\n290\r\n1\r\n310\r\n0aFF\r\n280\r\n-1\r\n999\r\nnote\r\n")
            .unwrap();
        assert_eq!(
            gs,
            vec![
                Group::text(0, "LINE"),
                Group::double(10, 1.5),
                Group::int(70, -3),
                Group {
                    code: 290,
                    value: Value::Bool(true)
                },
                Group {
                    code: 310,
                    value: Value::Bytes(vec![10, 255])
                },
                Group::int(280, 255),
            ]
        );
    }

    #[test]
    fn malformed_ascii_reports_the_line() {
        let e = groups(b"0\nX\n10\nabc\n").unwrap_err();
        assert_eq!(e.code(), "io.dxf.value_invalid");
        assert_eq!(
            e.details().get("line"),
            Some(&arc_base::error::Value::Int(3))
        );
        assert_eq!(
            groups(b"x\n").unwrap_err().code(),
            "io.dxf.group_code_invalid"
        );
        assert_eq!(groups(b"10\n").unwrap_err().code(), "io.dxf.truncated");
        assert_eq!(
            groups(b"70\n40000\n").unwrap_err().code(),
            "io.dxf.value_invalid"
        );
        assert_eq!(
            groups(b"40\nNaN\n").unwrap_err().code(),
            "io.dxf.value_invalid"
        );
        assert_eq!(
            groups(b"1\n\xff\n").unwrap_err().code(),
            "io.dxf.text_invalid"
        );
    }

    #[test]
    fn binary_groups_and_their_offsets() {
        let mut data = SENTINEL.to_vec();
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(b"SECTION\0");
        data.extend_from_slice(&10i16.to_le_bytes());
        data.extend_from_slice(&2.5f64.to_le_bytes());
        data.extend_from_slice(&40i16.to_le_bytes());
        data.extend_from_slice(&[1, 2]);
        let limits = Limits::STRICT;
        let mut t = Tokens::new(&data, Decoder::Utf8, &limits);
        assert_eq!(t.next().unwrap().unwrap().0, Group::text(0, "SECTION"));
        let (g, at) = t.next().unwrap().unwrap();
        assert_eq!((g, at), (Group::double(10, 2.5), Position::Offset(32)));
        assert_eq!(t.next().unwrap_err().code(), "io.dxf.truncated");
    }
}
