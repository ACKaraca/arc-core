//! The STEP physical file reader: ISO 10303-21, clear text encoding, over a byte buffer.
//!
//! The grammar is read by a cursor that never indexes the buffer directly. Nesting is recursive
//! but bounded by a hard cap ([`MAX_NESTING`]) in addition to the caller's `nesting_depth`, so the
//! stack cannot grow with the input. Every count is checked against its limit before the element
//! is stored.

use std::collections::BTreeSet;

use arc_base::{ErrorCode, Limits, Result};

use crate::error::{at_offset, codes, limit};
use crate::model::{Instance, Parameter, Record};
use crate::options::Control;
use crate::text;

/// The deepest nesting of lists and typed parameters the parser follows, whatever the profile
/// allows. It bounds the recursion of the parser's stack.
pub(crate) const MAX_NESTING: u32 = 256;

/// The header records and the instances of a physical file.
pub(crate) struct Parsed {
    pub(crate) header: Vec<Record>,
    pub(crate) instances: Vec<Instance>,
}

/// Parses a physical file held in `data` under `limits`.
pub(crate) fn parse(data: &[u8], limits: &Limits, control: &mut Control<'_>) -> Result<Parsed> {
    let length = to_u64(data.len());
    if length > limits.input_bytes {
        return Err(limit("input_bytes", length));
    }
    let mut parser = Parser {
        data,
        pos: 0,
        limits: *limits,
        depth_limit: limits.nesting_depth.min(MAX_NESTING),
        control,
    };
    parser.keyword("ISO-10303-21")?;
    parser.expect(b';')?;
    let header = parser.header()?;
    parser.keyword("DATA")?;
    parser.expect(b';')?;
    let instances = parser.instances()?;
    parser.keyword("END-ISO-10303-21")?;
    parser.expect(b';')?;
    parser.skip_space()?;
    if parser.pos < data.len() {
        return Err(parser.fail(codes::IFC_SYNTAX));
    }
    Ok(Parsed { header, instances })
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

struct Parser<'a, 'c, 'p> {
    data: &'a [u8],
    pos: usize,
    limits: Limits,
    depth_limit: u32,
    control: &'c mut Control<'p>,
}

impl<'a> Parser<'a, '_, '_> {
    fn fail(&self, code: ErrorCode) -> arc_base::Error {
        at_offset(code, to_u64(self.pos))
    }

    /// The error for the end of the input, or `code` when the input has more bytes.
    fn fail_here(&self, code: ErrorCode) -> arc_base::Error {
        if self.pos >= self.data.len() {
            self.fail(codes::IFC_TRUNCATED)
        } else {
            self.fail(code)
        }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.data.get(self.pos + offset).copied()
    }

    /// Skips white space and comments (`/* ... */`). An unterminated comment is truncation.
    fn skip_space(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.pos += 1,
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    let body = self.pos + 2;
                    let end = self
                        .data
                        .get(body..)
                        .and_then(|rest| rest.windows(2).position(|w| w == b"*/"))
                        .ok_or_else(|| self.fail(codes::IFC_TRUNCATED))?;
                    self.pos = body + end + 2;
                }
                _ => return Ok(()),
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        self.skip_space()?;
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.fail_here(codes::IFC_SYNTAX))
        }
    }

    /// Reads an identifier: a letter, then letters, digits, `_` or `-`.
    fn ident(&mut self) -> Result<&'a str> {
        self.skip_space()?;
        let data: &'a [u8] = self.data;
        let start = self.pos;
        match self.peek() {
            Some(b) if b.is_ascii_alphabetic() => {}
            _ => return Err(self.fail_here(codes::IFC_SYNTAX)),
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let bytes = data.get(start..self.pos).unwrap_or_default();
        std::str::from_utf8(bytes).map_err(|_| self.fail(codes::IFC_SYNTAX))
    }

    /// Reads the section keyword `expected`. A different word is a structure error; the end of
    /// the input is truncation.
    fn keyword(&mut self, expected: &str) -> Result<()> {
        self.skip_space()?;
        if self.peek().is_none() {
            return Err(self.fail(codes::IFC_TRUNCATED));
        }
        let found = self
            .ident()
            .map_err(|_| self.fail(codes::IFC_STRUCTURE_INVALID))?;
        if found == expected {
            Ok(())
        } else {
            Err(self.fail(codes::IFC_STRUCTURE_INVALID))
        }
    }

    /// The header section: records up to `ENDSEC;`.
    fn header(&mut self) -> Result<Vec<Record>> {
        self.keyword("HEADER")?;
        self.expect(b';')?;
        let mut records = Vec::new();
        loop {
            self.skip_space()?;
            if self.peek().is_none() {
                return Err(self.fail(codes::IFC_TRUNCATED));
            }
            let start = self.pos;
            let word = self.ident()?;
            if word == "ENDSEC" {
                self.expect(b';')?;
                return Ok(records);
            }
            self.pos = start;
            check_count(records.len(), self.limits.array_length, "array_length")?;
            records.push(self.record()?);
            self.expect(b';')?;
        }
    }

    /// The data section: instances up to `ENDSEC;`.
    fn instances(&mut self) -> Result<Vec<Instance>> {
        let mut instances: Vec<Instance> = Vec::new();
        let mut names = BTreeSet::new();
        loop {
            self.skip_space()?;
            match self.peek() {
                None => return Err(self.fail(codes::IFC_TRUNCATED)),
                Some(b'#') => {
                    let instance = self.instance()?;
                    if !names.insert(instance.name) {
                        return Err(self.fail(codes::IFC_INSTANCE_DUPLICATE));
                    }
                    check_count(instances.len(), self.limits.record_count, "record_count")?;
                    instances.push(instance);
                    self.control
                        .step(to_u64(self.pos), to_u64(self.data.len()))?;
                }
                Some(_) => {
                    let start = self.pos;
                    let word = self.ident()?;
                    if word == "ENDSEC" {
                        self.expect(b';')?;
                        return Ok(instances);
                    }
                    self.pos = start;
                    return Err(self.fail(codes::IFC_STRUCTURE_INVALID));
                }
            }
        }
    }

    /// `#n = record;` or `#n = (record record ...);`
    fn instance(&mut self) -> Result<Instance> {
        self.pos += 1;
        let name = self.number_u64()?;
        self.expect(b'=')?;
        self.skip_space()?;
        let mut records = Vec::new();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            loop {
                self.skip_space()?;
                match self.peek() {
                    Some(b')') => {
                        self.pos += 1;
                        break;
                    }
                    Some(b) if b.is_ascii_alphabetic() => {
                        check_count(records.len(), self.limits.array_length, "array_length")?;
                        records.push(self.record()?);
                    }
                    _ => return Err(self.fail_here(codes::IFC_SYNTAX)),
                }
            }
        } else {
            records.push(self.record()?);
        }
        self.expect(b';')?;
        Ok(Instance { name, records })
    }

    /// `TYPE(parameters)`, the type name in upper case.
    fn record(&mut self) -> Result<Record> {
        let type_name = self.ident()?.to_ascii_uppercase();
        self.expect(b'(')?;
        let parameters = self.items(1)?;
        Ok(Record {
            type_name,
            parameters,
        })
    }

    /// Parameters up to and including the closing parenthesis, which the caller has opened
    /// at nesting level `depth`.
    fn items(&mut self, depth: u32) -> Result<Vec<Parameter>> {
        let mut items = Vec::new();
        self.skip_space()?;
        if self.peek() == Some(b')') {
            self.pos += 1;
            return Ok(items);
        }
        loop {
            check_count(items.len(), self.limits.array_length, "array_length")?;
            let value = self.parameter(depth)?;
            items.push(value);
            self.control
                .step(to_u64(self.pos), to_u64(self.data.len()))?;
            self.skip_space()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b')') => {
                    self.pos += 1;
                    return Ok(items);
                }
                _ => return Err(self.fail_here(codes::IFC_SYNTAX)),
            }
        }
    }

    /// One parameter; `depth` is the number of parentheses open around it.
    fn parameter(&mut self, depth: u32) -> Result<Parameter> {
        self.skip_space()?;
        let Some(byte) = self.peek() else {
            return Err(self.fail(codes::IFC_TRUNCATED));
        };
        match byte {
            b'(' => {
                let inner = self.enter(depth)?;
                self.pos += 1;
                Ok(Parameter::List(self.items(inner)?))
            }
            b'\'' => self.string(),
            b'"' => self.binary(),
            b'#' => {
                self.pos += 1;
                Ok(Parameter::Reference(self.number_u64()?))
            }
            b'$' => {
                self.pos += 1;
                Ok(Parameter::Omitted)
            }
            b'*' => {
                self.pos += 1;
                Ok(Parameter::Derived)
            }
            b'.' if self.peek_at(1).is_some_and(|b| b.is_ascii_alphabetic()) => self.enumeration(),
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.number(),
            b if b.is_ascii_alphabetic() => {
                let type_name = self.ident()?.to_ascii_uppercase();
                self.expect(b'(')?;
                let inner = self.enter(depth)?;
                let value = self.parameter(inner)?;
                self.expect(b')')?;
                Ok(Parameter::Typed {
                    type_name,
                    value: Box::new(value),
                })
            }
            _ => Err(self.fail(codes::IFC_SYNTAX)),
        }
    }

    /// The nesting level of a parenthesis opened inside `depth`, refused past the limit.
    fn enter(&self, depth: u32) -> Result<u32> {
        let inner = depth.saturating_add(1);
        if inner > self.depth_limit {
            Err(limit("nesting_depth", u64::from(inner)))
        } else {
            Ok(inner)
        }
    }

    /// A string: the bytes between apostrophes, `''` standing for one apostrophe.
    fn string(&mut self) -> Result<Parameter> {
        let data: &'a [u8] = self.data;
        self.pos += 1;
        let start = self.pos;
        loop {
            match self.peek() {
                None => return Err(self.fail(codes::IFC_TRUNCATED)),
                Some(b'\'') if self.peek_at(1) == Some(b'\'') => self.pos += 2,
                Some(b'\'') => break,
                Some(_) => self.pos += 1,
            }
        }
        let raw = data.get(start..self.pos).unwrap_or_default();
        self.pos += 1;
        check_bytes(raw.len(), self.limits.string_bytes)?;
        Ok(Parameter::Text(text::decode(raw, to_u64(start))?))
    }

    /// A binary literal: `"` then a digit from 0 to 3, then hexadecimal digits, then `"`.
    fn binary(&mut self) -> Result<Parameter> {
        let data: &'a [u8] = self.data;
        self.pos += 1;
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b'"' {
                break;
            }
            self.pos += 1;
        }
        if self.peek().is_none() {
            return Err(self.fail(codes::IFC_TRUNCATED));
        }
        let raw = data.get(start..self.pos).unwrap_or_default();
        self.pos += 1;
        check_bytes(raw.len(), self.limits.string_bytes)?;
        let valid = raw.first().is_some_and(|b| (b'0'..=b'3').contains(b))
            && raw
                .iter()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b));
        if !valid {
            return Err(at_offset(codes::IFC_VALUE_INVALID, to_u64(start)));
        }
        Ok(Parameter::Binary(String::from_utf8_lossy(raw).into_owned()))
    }

    /// An enumeration such as `.T.`: letters, digits and `_` between two dots.
    fn enumeration(&mut self) -> Result<Parameter> {
        let data: &'a [u8] = self.data;
        self.pos += 1;
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name = data.get(start..self.pos).unwrap_or_default();
        if self.peek() != Some(b'.') {
            return Err(self.fail_here(codes::IFC_VALUE_INVALID));
        }
        self.pos += 1;
        check_bytes(name.len(), self.limits.string_bytes)?;
        Ok(Parameter::Enumeration(
            String::from_utf8_lossy(name).into_owned(),
        ))
    }

    /// An integer or a real. A real has a point, an exponent with `E`, or both; a point needs
    /// at least one digit before or after it. Integers and reals must fit their types.
    fn number(&mut self) -> Result<Parameter> {
        let data: &'a [u8] = self.data;
        let start = self.pos;
        let mut digits = 0usize;
        let mut real = false;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        digits += self.digit_run();
        if self.peek() == Some(b'.') {
            real = true;
            self.pos += 1;
            digits += self.digit_run();
        }
        if self.peek() == Some(b'E') {
            // The grammar of a real requires its point; an exponent alone is not a real.
            if !real {
                return Err(at_offset(codes::IFC_VALUE_INVALID, to_u64(start)));
            }
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digit_run() == 0 {
                return Err(at_offset(codes::IFC_VALUE_INVALID, to_u64(start)));
            }
        }
        let token = data.get(start..self.pos).unwrap_or_default();
        let invalid = || at_offset(codes::IFC_VALUE_INVALID, to_u64(start));
        if digits == 0 {
            return Err(invalid());
        }
        let text = std::str::from_utf8(token).map_err(|_| invalid())?;
        if real {
            let value = text
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(invalid)?;
            Ok(Parameter::Real(value))
        } else {
            text.parse::<i64>()
                .map(Parameter::Integer)
                .map_err(|_| invalid())
        }
    }

    /// Skips ASCII digits and returns how many there were.
    fn digit_run(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    /// A non-empty run of digits as an unsigned number.
    fn number_u64(&mut self) -> Result<u64> {
        let data: &'a [u8] = self.data;
        let start = self.pos;
        if self.digit_run() == 0 {
            return Err(self.fail_here(codes::IFC_SYNTAX));
        }
        let digits = data.get(start..self.pos).unwrap_or_default();
        let mut value: u64 = 0;
        for &digit in digits {
            let step = u64::from(digit - b'0');
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(step))
                .ok_or_else(|| at_offset(codes::IFC_VALUE_INVALID, to_u64(start)))?;
        }
        Ok(value)
    }
}

/// Refuses a string whose length exceeds `string_bytes`.
fn check_bytes(length: usize, max: u64) -> Result<()> {
    let length = to_u64(length);
    if length > max {
        Err(limit("string_bytes", length))
    } else {
        Ok(())
    }
}

/// Refuses one more element when the list already holds `max` of them.
fn check_count(held: usize, max: u64, name: &'static str) -> Result<()> {
    let next = to_u64(held).saturating_add(1);
    if next > max {
        Err(limit(name, next))
    } else {
        Ok(())
    }
}
