//! The STEP physical file reader: ISO 10303-21, clear text encoding, over a byte buffer.
//!
//! The grammar is read by a cursor that never indexes the buffer directly. Nesting is recursive
//! but bounded by a hard cap ([`MAX_NESTING`]) in addition to the caller's `nesting_depth`, so the
//! stack cannot grow with the input. Every count is checked against its limit before the element
//! is stored, and the parser keeps an estimate of the memory it allocates, checked against
//! `memory_bytes`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use arc_base::{ErrorCode, Limits, Result};

use crate::error::{at_offset, codes, limit};
use crate::model::{Instance, Parameter, Record};
use crate::options::Control;
use crate::text;

/// The deepest nesting of lists and typed parameters the parser follows, whatever the profile
/// allows. It bounds the recursion of the parser's stack.
pub(crate) const MAX_NESTING: u32 = 64;

/// The longest word the parser accepts: a keyword, a section name or an entity type name.
pub(crate) const MAX_WORD_BYTES: usize = 255;

/// Estimated bytes of one parameter: its enumeration slot and its share of the list holding it.
pub(crate) const PARAMETER_BYTES: u64 = 48;

/// Estimated bytes of one record, besides its type name.
pub(crate) const RECORD_BYTES: u64 = 64;

/// Estimated bytes of one instance: its name, its index entry, its record list and its byte span.
pub(crate) const INSTANCE_BYTES: u64 = 120;

/// What the reader saw that is not an error but is reported in the loss report.
#[derive(Debug, Default)]
pub(crate) struct Notes {
    /// Bytes of strings that are not UTF-8 and were read as ISO 8859-1.
    pub(crate) text_latin1: u64,
    /// Raw tab, carriage return and line feed bytes kept inside strings.
    pub(crate) text_controls: u64,
    /// Byte order mark and padding bytes ignored outside the sections.
    pub(crate) ignored_bytes: u64,
    /// Other bytes after `END-ISO-10303-21;`, ignored.
    pub(crate) trailing_bytes: u64,
    /// Archive entries skipped as metadata; set by the archive reader.
    pub(crate) skipped_entries: u64,
    /// Sections other than `DATA` that were skipped, by section name.
    pub(crate) sections: BTreeMap<&'static str, u64>,
}

/// The header records and the instances of a physical file, each with the byte span of its
/// source text (from the first byte of its keyword or `#` to its `;`, both included).
pub(crate) struct Parsed {
    pub(crate) header: Vec<Record>,
    pub(crate) header_spans: Vec<Range<usize>>,
    pub(crate) instances: Vec<Instance>,
    pub(crate) spans: Vec<Range<usize>>,
    /// The offset of the `ENDSEC` that closes the last data section.
    pub(crate) data_end: usize,
    pub(crate) estimated_bytes: u64,
    pub(crate) notes: Notes,
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
        used: 0,
        names: BTreeSet::new(),
        notes: Notes::default(),
        data_end: 0,
    };
    parser.byte_order_mark();
    parser.keyword("ISO-10303-21")?;
    parser.expect(b';')?;
    let (header, header_spans) = parser.header()?;
    let mut instances = Vec::new();
    let mut spans = Vec::new();
    let mut data_sections = 0u64;
    loop {
        parser.skip_space()?;
        if parser.peek().is_none() {
            return Err(parser.fail(codes::IFC_TRUNCATED));
        }
        let at = parser.pos;
        let word = parser
            .word(true, false)
            .map_err(|_| at_offset(codes::IFC_STRUCTURE_INVALID, to_u64(at)))?;
        match word {
            "DATA" => {
                parser.data_section(&mut instances, &mut spans)?;
                data_sections = data_sections.saturating_add(1);
            }
            "ANCHOR" => parser.skipped_section("ANCHOR")?,
            "REFERENCE" => parser.skipped_section("REFERENCE")?,
            "SIGNATURE" => parser.skipped_section("SIGNATURE")?,
            "END-ISO-10303-21" => break,
            _ => return Err(at_offset(codes::IFC_STRUCTURE_INVALID, to_u64(at))),
        }
    }
    if data_sections == 0 {
        return Err(parser.fail(codes::IFC_STRUCTURE_INVALID));
    }
    parser.expect(b';')?;
    parser.trailer();
    Ok(Parsed {
        header,
        header_spans,
        instances,
        spans,
        data_end: parser.data_end,
        estimated_bytes: parser.used,
        notes: parser.notes,
    })
}

/// Parses one instance statement, `#n=TYPE(...);`, held alone in `statement`, for the streaming
/// reader. Errors carry offsets relative to the statement.
pub(crate) fn parse_instance_statement(
    statement: &[u8],
    limits: &Limits,
    control: &mut Control<'_>,
) -> Result<Instance> {
    let mut parser = Parser::alone(statement, limits, control);
    parser.skip_space()?;
    if parser.peek() != Some(b'#') {
        return Err(parser.fail(codes::IFC_SYNTAX));
    }
    let instance = parser.instance()?;
    parser.alone_done()?;
    Ok(instance)
}

/// Parses one header record statement, `FILE_NAME(...);`, held alone in `statement`.
pub(crate) fn parse_header_statement(
    statement: &[u8],
    limits: &Limits,
    control: &mut Control<'_>,
) -> Result<Record> {
    let mut parser = Parser::alone(statement, limits, control);
    let record = parser.record()?;
    parser.expect(b';')?;
    parser.alone_done()?;
    Ok(record)
}

/// The keyword that starts a statement: letters, digits, `_` and `-`, in upper case, and the byte
/// offset where it ends. `None` when the statement does not start with one.
pub(crate) fn leading_keyword(statement: &[u8]) -> Option<(String, usize)> {
    let end = statement
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-'))
        .unwrap_or(statement.len());
    if end == 0 || !statement.first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let word = std::str::from_utf8(statement.get(..end)?).ok()?;
    Some((word.to_ascii_uppercase(), end))
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
    /// The estimate of the memory the parser has allocated so far.
    used: u64,
    /// The instance names read so far, across all data sections.
    names: BTreeSet<u64>,
    notes: Notes,
    /// The offset of the `ENDSEC` of the last data section read so far.
    data_end: usize,
}

impl<'a, 'c, 'p> Parser<'a, 'c, 'p> {
    /// A parser over one statement, with fresh counters.
    fn alone(data: &'a [u8], limits: &Limits, control: &'c mut Control<'p>) -> Self {
        Self {
            data,
            pos: 0,
            limits: *limits,
            depth_limit: limits.nesting_depth.min(MAX_NESTING),
            control,
            used: 0,
            names: BTreeSet::new(),
            notes: Notes::default(),
            data_end: 0,
        }
    }

    /// Checks that nothing but space follows a statement parsed alone.
    fn alone_done(&mut self) -> Result<()> {
        self.skip_space()?;
        if self.peek().is_some() {
            return Err(self.fail(codes::IFC_SYNTAX));
        }
        Ok(())
    }
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

    /// Adds `bytes` to the memory estimate and refuses when the estimate passes `memory_bytes`.
    fn charge(&mut self, bytes: u64) -> Result<()> {
        self.used = self.used.saturating_add(bytes);
        match self.limits.memory_bytes {
            Some(max) if self.used > max => Err(limit("memory_bytes", self.used)),
            _ => Ok(()),
        }
    }

    /// Skips a UTF-8 byte order mark before the first keyword.
    fn byte_order_mark(&mut self) {
        if self.data.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.pos = 3;
            self.notes.ignored_bytes = self.notes.ignored_bytes.saturating_add(3);
        }
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

    /// Reads a word: a letter, then letters, digits and `_`, and `-` when `dash` is set. When
    /// `user_defined` is set the word may start with `!`, which is part of it. A word longer than
    /// [`MAX_WORD_BYTES`] is a syntax error; one longer than `string_bytes` is a limit error.
    fn word(&mut self, dash: bool, user_defined: bool) -> Result<&'a str> {
        self.skip_space()?;
        let data: &'a [u8] = self.data;
        let start = self.pos;
        if user_defined && self.peek() == Some(b'!') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b) if b.is_ascii_alphabetic() => {}
            _ => return Err(self.fail_here(codes::IFC_SYNTAX)),
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || (dash && b == b'-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let length = self.pos.saturating_sub(start);
        if length > MAX_WORD_BYTES {
            return Err(at_offset(codes::IFC_SYNTAX, to_u64(start)));
        }
        check_bytes(length, self.limits.string_bytes)?;
        let bytes = data.get(start..self.pos).unwrap_or_default();
        std::str::from_utf8(bytes).map_err(|_| self.fail(codes::IFC_SYNTAX))
    }

    /// An entity type name or an attribute-like identifier: [`Parser::word`] without `-`, with
    /// the user-defined `!` form allowed.
    fn ident(&mut self) -> Result<&'a str> {
        self.word(false, true)
    }

    /// Reads the section keyword `expected`. A different word is a structure error; the end of
    /// the input is truncation.
    fn keyword(&mut self, expected: &str) -> Result<()> {
        self.skip_space()?;
        if self.peek().is_none() {
            return Err(self.fail(codes::IFC_TRUNCATED));
        }
        let found = self
            .word(true, false)
            .map_err(|_| self.fail(codes::IFC_STRUCTURE_INVALID))?;
        if found == expected {
            Ok(())
        } else {
            Err(self.fail(codes::IFC_STRUCTURE_INVALID))
        }
    }

    /// The header section: records up to `ENDSEC;`.
    fn header(&mut self) -> Result<(Vec<Record>, Vec<Range<usize>>)> {
        self.keyword("HEADER")?;
        self.expect(b';')?;
        let mut records = Vec::new();
        let mut spans = Vec::new();
        loop {
            self.skip_space()?;
            if self.peek().is_none() {
                return Err(self.fail(codes::IFC_TRUNCATED));
            }
            let start = self.pos;
            let word = self.ident()?;
            if word == "ENDSEC" {
                self.expect(b';')?;
                return Ok((records, spans));
            }
            self.pos = start;
            check_count(records.len(), self.limits.array_length, "array_length")?;
            records.push(self.record()?);
            self.expect(b';')?;
            spans.push(start..self.pos);
        }
    }

    /// A data section after its keyword: optional parameters, `;`, instances up to `ENDSEC;`.
    /// Instances of every data section go into one list and one set of names.
    fn data_section(
        &mut self,
        out: &mut Vec<Instance>,
        spans: &mut Vec<Range<usize>>,
    ) -> Result<()> {
        self.skip_space()?;
        if self.peek() == Some(b'(') {
            self.pos += 1;
            self.items(1)?;
        }
        self.expect(b';')?;
        self.instances(out, spans)
    }

    /// A section that is not read (`ANCHOR`, `REFERENCE`, `SIGNATURE`): its statements are parsed,
    /// so that its strings and nesting are checked, and then dropped.
    fn skipped_section(&mut self, name: &'static str) -> Result<()> {
        self.skip_space()?;
        if self.peek() == Some(b'(') {
            self.pos += 1;
            self.items(1)?;
        }
        self.expect(b';')?;
        loop {
            self.skip_space()?;
            match self.peek() {
                None => return Err(self.fail(codes::IFC_TRUNCATED)),
                Some(b'#') => {
                    self.instance()?;
                }
                Some(_) => {
                    let start = self.pos;
                    if self.ident()? == "ENDSEC" {
                        self.expect(b';')?;
                        break;
                    }
                    self.pos = start;
                    self.record()?;
                    self.expect(b';')?;
                }
            }
            self.control
                .step(to_u64(self.pos), to_u64(self.data.len()))?;
        }
        let count = self.notes.sections.entry(name).or_default();
        *count = count.saturating_add(1);
        Ok(())
    }

    /// The instances of a data section, up to `ENDSEC;`.
    fn instances(&mut self, out: &mut Vec<Instance>, spans: &mut Vec<Range<usize>>) -> Result<()> {
        loop {
            self.skip_space()?;
            match self.peek() {
                None => return Err(self.fail(codes::IFC_TRUNCATED)),
                Some(b'#') => {
                    let start = self.pos;
                    let instance = self.instance()?;
                    if !self.names.insert(instance.name) {
                        return Err(self.fail(codes::IFC_INSTANCE_DUPLICATE));
                    }
                    check_count(out.len(), self.limits.record_count, "record_count")?;
                    out.push(instance);
                    spans.push(start..self.pos);
                    self.control
                        .step(to_u64(self.pos), to_u64(self.data.len()))?;
                }
                Some(_) => {
                    let start = self.pos;
                    let word = self.ident()?;
                    if word == "ENDSEC" {
                        self.data_end = start;
                        self.expect(b';')?;
                        return Ok(());
                    }
                    self.pos = start;
                    return Err(self.fail(codes::IFC_STRUCTURE_INVALID));
                }
            }
        }
    }

    /// `#n = record;` or `#n = (record record ...);`. A complex instance has at least one record.
    fn instance(&mut self) -> Result<Instance> {
        self.pos += 1;
        let name = self.number_u64()?;
        self.expect(b'=')?;
        self.charge(INSTANCE_BYTES)?;
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
                    Some(b) if b.is_ascii_alphabetic() || b == b'!' => {
                        check_count(records.len(), self.limits.array_length, "array_length")?;
                        records.push(self.record()?);
                    }
                    _ => return Err(self.fail_here(codes::IFC_SYNTAX)),
                }
            }
            if records.is_empty() {
                return Err(self.fail(codes::IFC_SYNTAX));
            }
        } else {
            records.push(self.record()?);
        }
        self.expect(b';')?;
        Ok(Instance { name, records })
    }

    /// `TYPE(parameters)`, the type name in upper case.
    fn record(&mut self) -> Result<Record> {
        let raw = self.ident()?;
        self.charge(RECORD_BYTES.saturating_add(to_u64(raw.len())))?;
        let type_name = raw.to_ascii_uppercase();
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
        self.charge(PARAMETER_BYTES)?;
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
            b if b.is_ascii_alphabetic() || b == b'!' => self.typed(depth),
            _ => Err(self.fail(codes::IFC_SYNTAX)),
        }
    }

    /// A typed parameter `TYPE(value)`. It lives apart from [`Parser::parameter`] so that the
    /// recursion through nested parameters keeps a small stack frame.
    #[inline(never)]
    fn typed(&mut self, depth: u32) -> Result<Parameter> {
        let raw = self.ident()?;
        self.charge(to_u64(raw.len()))?;
        let type_name = raw.to_ascii_uppercase();
        self.expect(b'(')?;
        let inner = self.enter(depth)?;
        let value = self.parameter(inner)?;
        self.expect(b')')?;
        Ok(Parameter::Typed {
            type_name,
            value: Box::new(value),
        })
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
        self.charge(to_u64(raw.len()))?;
        let decoded = text::decode(raw, to_u64(start))?;
        self.notes.text_latin1 = self.notes.text_latin1.saturating_add(decoded.latin1);
        self.notes.text_controls = self.notes.text_controls.saturating_add(decoded.controls);
        Ok(Parameter::Text(decoded.text))
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
        self.charge(to_u64(raw.len()))?;
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
        self.charge(to_u64(name.len()))?;
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

    /// After `END-ISO-10303-21;`: white space is ignored silently, NUL and 0x1A padding are counted
    /// as ignored bytes, and any other byte is counted as trailing data and ignored.
    fn trailer(&mut self) {
        let data: &'a [u8] = self.data;
        let rest = data.get(self.pos..).unwrap_or_default();
        for &byte in rest {
            let counter = match byte {
                b' ' | b'\t' | b'\r' | b'\n' => continue,
                0 | 0x1A => &mut self.notes.ignored_bytes,
                _ => &mut self.notes.trailing_bytes,
            };
            *counter = counter.saturating_add(1);
        }
        self.pos = self.data.len();
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
