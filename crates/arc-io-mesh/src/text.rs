//! Lines, numbers and text of the ASCII forms: bounded line splitting, strict number parsing and
//! the shortest round-trip output of numbers.

use arc_base::{Limits, Result};

use crate::error::{at_line, codes, limit};

/// A length as `u64`, saturating.
pub(crate) fn len_u64(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

/// Fails with `io.limit_exceeded` when the input is longer than `Limits::input_bytes`.
pub(crate) fn check_input(length: usize, limits: &Limits) -> Result<()> {
    let length = len_u64(length);
    if length > limits.input_bytes {
        Err(limit("input_bytes", length))
    } else {
        Ok(())
    }
}

/// The lines of a byte buffer without their line breaks, numbered from one. A line longer than
/// `Limits::string_bytes` ends the iteration with `io.limit_exceeded`.
pub(crate) struct Lines<'a> {
    data: &'a [u8],
    rest: &'a [u8],
    number: u64,
    max_len: u64,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(data: &'a [u8], limits: &Limits) -> Self {
        Self {
            data,
            rest: data,
            number: 0,
            max_len: limits.string_bytes,
        }
    }

    /// The byte offset of the first unread line.
    pub(crate) fn offset(&self) -> u64 {
        len_u64(self.data.len() - self.rest.len())
    }

    /// The number of the last line returned.
    pub(crate) fn number(&self) -> u64 {
        self.number
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = Result<(u64, &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let (mut line, rest): (&'a [u8], &'a [u8]) =
            match self.rest.iter().position(|&b| b == b'\n') {
                Some(at) => (
                    self.rest.get(..at).unwrap_or_default(),
                    self.rest.get(at + 1..).unwrap_or_default(),
                ),
                None => (self.rest, &[]),
            };
        self.rest = rest;
        self.number = self.number.saturating_add(1);
        if let Some(stripped) = line.strip_suffix(b"\r") {
            line = stripped;
        }
        let length = len_u64(line.len());
        if length > self.max_len {
            self.rest = &[];
            return Some(Err(limit("string_bytes", length)));
        }
        Some(Ok((self.number, line)))
    }
}

/// The text of one line, which must be valid UTF-8.
pub(crate) fn text(line: u64, bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| at_line(codes::MESH_TEXT_INVALID, line))
}

/// A finite `f64` parsed from a token.
pub(crate) fn float64(line: u64, token: &str) -> Result<f64> {
    token
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| at_line(codes::MESH_VALUE_INVALID, line))
}

/// A finite `f32` parsed from a token.
pub(crate) fn float32(line: u64, token: &str) -> Result<f32> {
    token
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| at_line(codes::MESH_VALUE_INVALID, line))
}

/// An integer parsed from a token.
pub(crate) fn integer(line: u64, token: &str) -> Result<i64> {
    token
        .parse::<i64>()
        .map_err(|_| at_line(codes::MESH_VALUE_INVALID, line))
}

/// The shortest text of a finite `f64` that reads back to the same value: plain decimal in the
/// common range, exponent form outside it.
pub(crate) fn text64(value: f64) -> String {
    let magnitude = value.abs();
    if value == 0.0 || (1e-4..1e15).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

/// The shortest text of a finite `f32` that reads back to the same value.
pub(crate) fn text32(value: f32) -> String {
    let magnitude = value.abs();
    if value == 0.0 || (1e-4..1e15).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

/// Rounds a model value to single precision.
#[expect(
    clippy::cast_possible_truncation,
    reason = "narrowing to single precision is the purpose"
)]
pub(crate) fn narrow(value: f64) -> f32 {
    value as f32
}

/// Fails with `io.mesh.write_value_invalid` unless the text can be written on one line: no line
/// break or NUL character and no leading or trailing white space.
pub(crate) fn line_text(value: &str) -> Result<()> {
    let bad = value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) || value.trim() != value;
    if bad {
        Err(arc_base::Error::new(codes::MESH_WRITE_VALUE_INVALID))
    } else {
        Ok(())
    }
}
