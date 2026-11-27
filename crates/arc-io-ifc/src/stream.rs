//! Streaming read (CORE-IO-017): an IFC physical file read from a [`Read`] source with memory
//! bounded by the window and the index, not by the file.
//!
//! [`scan`] reads the source once, in chunks, and hands each instance to a visitor as soon as it is
//! parsed; nothing is kept. [`StreamIndex::build`] does the same and keeps, per instance, its name,
//! the byte span of its source text and its entity types, about 80 bytes whatever the instance
//! holds. [`StreamFile`] couples an index with a [`Read`] + [`Seek`] source and parses an instance
//! on demand from its span, so `instance(name)` costs one seek and the parse of one record.
//!
//! The spans are the ones [`IfcFile::span`](crate::IfcFile::span) reports: from the `#` of the
//! instance to its `;`, as byte offsets in the physical file. The instances are the ones [`read`]
//! returns, parsed by the same code. A statement that never ends within the window limit (see
//! [`StreamLimits`]) is refused, so a hostile file cannot make the window grow without bound.
//!
//! Not streamed: an IFC-ZIP archive (an archive is read whole, then [`read`](crate::read) applies),
//! and the loss report (the views read an [`IfcFile`](crate::IfcFile)).

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;

use arc_base::{Cancel, Error, Limits, Result};

use crate::error::{at_offset, codes, limit};
use crate::model::{Instance, Parameter, Record, Schema};
use crate::options::Control;
use crate::step::{leading_keyword, parse_header_statement, parse_instance_statement};

/// The bytes read from the source at a time.
const CHUNK: usize = 64 * 1024;

/// The bytes the index charges for one instance: its entry, its name in the type lists and its
/// share of the type table.
const INDEX_BYTES: u64 = 96;

/// The bounds that are specific to streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimits {
    /// The longest statement, in bytes, the window holds. A statement longer than this is
    /// `io.limit_exceeded` (`string_bytes`).
    pub statement_bytes: u64,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            statement_bytes: 64 * 1024 * 1024,
        }
    }
}

/// What the scan saw besides the instances.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamSummary {
    /// The schema named by `FILE_SCHEMA`.
    pub schema: Schema,
    /// The header records, in file order, with their byte spans.
    pub header: Vec<(Record, Range<u64>)>,
    /// The number of instances.
    pub instances: u64,
    /// The bytes read.
    pub bytes: u64,
    /// The largest number of bytes the window held at once.
    pub peak_window_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Start,
    Top,
    Header,
    Data,
    Skipped,
    End,
}

/// The scanner state across chunks: whether the cursor is inside a string or a comment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lex {
    Code,
    /// After a `/` in code: a `*` opens a comment.
    Slash,
    Str,
    Comment,
    /// After a `*` in a comment: a `/` closes it.
    CommentStar,
}

struct Window<'r, R: Read> {
    source: &'r mut R,
    buffer: Vec<u8>,
    /// File offset of `buffer[0]`.
    base: u64,
    /// Start of the unconsumed bytes in `buffer`.
    start: usize,
    /// Where the scan of the current statement has reached, and its state.
    scan: usize,
    lex: Lex,
    eof: bool,
    total: u64,
    peak: u64,
    limits: Limits,
    statement_max: u64,
}

impl<R: Read> Window<'_, R> {
    fn fill(&mut self) -> Result<()> {
        self.fill_with(true)
    }

    /// Reads more bytes; `compact` drops the consumed prefix first, which moves every offset into
    /// the buffer, so a caller that holds one passes `false`.
    fn fill_with(&mut self, compact: bool) -> Result<()> {
        // Drop the consumed prefix once it is at least as large as what is kept.
        if compact && self.start > 0 && self.start >= self.buffer.len() / 2 {
            self.buffer.drain(..self.start);
            self.base += self.start as u64;
            self.scan = self.scan.saturating_sub(self.start);
            self.start = 0;
        }
        let old = self.buffer.len();
        self.buffer.resize(old + CHUNK, 0);
        let mut got = 0;
        loop {
            match self.source.read(&mut self.buffer[old + got..]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    got += n;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.buffer.truncate(old);
                    return Err(Error::new(codes::IFC_TRUNCATED));
                }
            }
        }
        self.buffer.truncate(old + got);
        self.total += got as u64;
        if self.total > self.limits.input_bytes {
            return Err(limit("input_bytes", self.total));
        }
        self.peak = self.peak.max(self.buffer.len() as u64);
        Ok(())
    }

    /// The next statement: its byte range in the buffer (without leading space and comments) and
    /// its file offset; `None` at the end of the input when only space remains.
    fn next_statement(&mut self, cancel: &Cancel) -> Result<Option<(Range<usize>, u64)>> {
        // Skip space and comments before the statement.
        loop {
            while self.start < self.buffer.len() && self.buffer[self.start].is_ascii_whitespace() {
                self.start += 1;
            }
            if self.start + 1 < self.buffer.len()
                && self.buffer[self.start] == b'/'
                && self.buffer[self.start + 1] == b'*'
            {
                // A comment before a statement: find its end.
                let mut at = self.start + 2;
                loop {
                    if let Some(i) = self.buffer[at..].windows(2).position(|w| w == b"*/") {
                        self.start = at + i + 2;
                        break;
                    }
                    at = self.buffer.len().saturating_sub(1).max(self.start + 2);
                    if self.eof {
                        return Err(at_offset(
                            codes::IFC_TRUNCATED,
                            self.base + self.buffer.len() as u64,
                        ));
                    }
                    self.fill_with(false)?;
                    at = at.min(self.buffer.len());
                }
                continue;
            }
            if self.start < self.buffer.len() {
                // A `/` that is the last byte may begin a comment; read on.
                if self.buffer[self.start] == b'/'
                    && !self.eof
                    && self.start + 1 == self.buffer.len()
                {
                    self.fill()?;
                    continue;
                }
                break;
            }
            if self.eof {
                return Ok(None);
            }
            self.fill()?;
        }
        self.scan = self.start;
        self.lex = Lex::Code;
        loop {
            cancel.check()?;
            while self.scan < self.buffer.len() {
                let b = self.buffer[self.scan];
                self.scan += 1;
                if self.lex == Lex::Slash {
                    // A `/` that does not open a comment is an ordinary byte; look at this one
                    // as code.
                    self.lex = if b == b'*' { Lex::Comment } else { Lex::Code };
                    if b == b'*' {
                        continue;
                    }
                }
                match self.lex {
                    Lex::Code | Lex::Slash => match b {
                        b'\'' => self.lex = Lex::Str,
                        b'/' => self.lex = Lex::Slash,
                        b';' => {
                            let range = self.start..self.scan;
                            return Ok(Some((range, self.base + self.start as u64)));
                        }
                        _ => {}
                    },
                    Lex::Str => {
                        if b == b'\'' {
                            self.lex = Lex::Code;
                        }
                    }
                    Lex::Comment | Lex::CommentStar => {
                        self.lex = match (self.lex, b) {
                            (Lex::CommentStar, b'/') => Lex::Code,
                            (_, b'*') => Lex::CommentStar,
                            _ => Lex::Comment,
                        };
                    }
                }
            }
            if self.eof {
                return Err(at_offset(
                    codes::IFC_TRUNCATED,
                    self.base + self.scan as u64,
                ));
            }
            if (self.buffer.len() - self.start) as u64 > self.statement_max {
                return Err(limit(
                    "string_bytes",
                    (self.buffer.len() - self.start) as u64,
                ));
            }
            self.fill()?;
        }
    }
}

/// Reads `source` once and calls `visit` for each instance with its parsed form and byte span.
/// Nothing is kept: the memory is the window (a few chunks, or the longest statement) plus
/// whatever the visitor keeps.
///
/// # Errors
///
/// The errors of [`read`](crate::read) for malformed input, with offsets relative to the start of
/// the statement and the detail `statement_offset` with its place in the file;
/// `io.limit_exceeded` for `input_bytes`, `record_count` and `string_bytes` (a statement longer
/// than the window limit); `base.cancelled`; the error `visit` returns.
pub fn scan<R: Read>(
    source: &mut R,
    limits: &Limits,
    stream: &StreamLimits,
    cancel: &Cancel,
    mut visit: impl FnMut(&Instance, Range<u64>) -> Result<()>,
) -> Result<StreamSummary> {
    let mut window = Window {
        source,
        buffer: Vec::new(),
        base: 0,
        start: 0,
        scan: 0,
        lex: Lex::Code,
        eof: false,
        total: 0,
        peak: 0,
        limits: *limits,
        statement_max: stream.statement_bytes,
    };
    let mut control = Control::new(cancel, None, None, limits.wall_clock_seconds);
    let mut section = Section::Start;
    let mut header = Vec::new();
    let mut count = 0u64;
    let mut names = std::collections::BTreeSet::new();
    let mut schema = None;
    if window.buffer.is_empty() {
        window.fill()?;
        if window.buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
            window.start = 3;
        }
    }
    while let Some((range, offset)) = window.next_statement(cancel)? {
        let statement = &window.buffer[range.clone()];
        let at = |e: Error| {
            e.with_detail(
                "statement_offset",
                i64::try_from(offset).unwrap_or(i64::MAX),
            )
        };
        let keyword = leading_keyword(statement).map(|(word, _)| word);
        match (section, keyword.as_deref(), statement.first()) {
            (Section::Start, Some("ISO-10303-21"), _) => section = Section::Top,
            (Section::Start, _, _) => return Err(at(Error::new(codes::IFC_STRUCTURE_INVALID))),
            (Section::Top, Some("HEADER"), _) => section = Section::Header,
            (Section::Top, Some("DATA"), _) => section = Section::Data,
            (Section::Top, Some("ANCHOR" | "REFERENCE" | "SIGNATURE"), _) => {
                section = Section::Skipped
            }
            (Section::Top, Some("END-ISO-10303-21"), _) => section = Section::End,
            (Section::Top, _, _) => return Err(at(Error::new(codes::IFC_STRUCTURE_INVALID))),
            (Section::Header, Some("ENDSEC"), _)
            | (Section::Data | Section::Skipped, Some("ENDSEC"), _) => {
                section = Section::Top;
            }
            (Section::Header, _, _) => {
                let record = parse_header_statement(statement, limits, &mut control).map_err(at)?;
                if limits.array_length <= header.len() as u64 {
                    return Err(limit("array_length", header.len() as u64));
                }
                header.push((record, offset..offset + statement.len() as u64));
            }
            (Section::Data, _, Some(b'#')) => {
                let instance =
                    parse_instance_statement(statement, limits, &mut control).map_err(at)?;
                if !names.insert(instance.name) {
                    return Err(at(Error::new(codes::IFC_INSTANCE_DUPLICATE)));
                }
                if count >= limits.record_count {
                    return Err(limit("record_count", count));
                }
                count += 1;
                visit(&instance, offset..offset + statement.len() as u64)?;
            }
            (Section::Data, _, _) => return Err(at(Error::new(codes::IFC_STRUCTURE_INVALID))),
            (Section::Skipped, _, _) => {}
            (Section::End, _, _) => {}
        }
        window.start = range.end;
        control.step(window.base + window.start as u64, 0)?;
        if section == Section::End {
            break;
        }
    }
    if section != Section::End {
        return Err(at_offset(codes::IFC_TRUNCATED, window.total));
    }
    let schema = header
        .iter()
        .find(|(r, _)| r.type_name == "FILE_SCHEMA")
        .and_then(|(r, _)| match r.parameters.first() {
            Some(Parameter::List(names)) => match names.first() {
                Some(Parameter::Text(identifier)) => Some(Schema::from_identifier(identifier)),
                _ => None,
            },
            _ => None,
        })
        .or(schema.take())
        .ok_or_else(|| Error::new(codes::IFC_STRUCTURE_INVALID))?;
    Ok(StreamSummary {
        schema,
        header,
        instances: count,
        bytes: window.total,
        peak_window_bytes: window.peak,
    })
}

/// One indexed instance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    start: u64,
    end: u64,
}

/// The index of a physical file: for each instance the span of its source text, and the instances
/// of each entity type. See the module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamIndex {
    summary: StreamSummary,
    entries: BTreeMap<u64, Entry>,
    by_type: BTreeMap<String, Vec<u64>>,
    estimated_bytes: u64,
}

impl StreamIndex {
    /// Reads `source` once and indexes every instance.
    ///
    /// # Errors
    ///
    /// As [`scan`]; `io.limit_exceeded` for `memory_bytes` when the index outgrows it.
    pub fn build<R: Read>(
        source: &mut R,
        limits: &Limits,
        stream: &StreamLimits,
        cancel: &Cancel,
    ) -> Result<Self> {
        let mut entries = BTreeMap::new();
        let mut by_type: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut used = 0u64;
        let summary = scan(source, limits, stream, cancel, |instance, span| {
            used = used.saturating_add(INDEX_BYTES);
            if let Some(max) = limits.memory_bytes
                && used > max
            {
                return Err(limit("memory_bytes", used));
            }
            entries.insert(
                instance.name,
                Entry {
                    start: span.start,
                    end: span.end,
                },
            );
            for record in &instance.records {
                let names = by_type.entry(record.type_name.clone()).or_default();
                if names.last() != Some(&instance.name) {
                    names.push(instance.name);
                }
            }
            Ok(())
        })?;
        Ok(Self {
            summary,
            entries,
            by_type,
            estimated_bytes: used,
        })
    }

    /// The schema named by `FILE_SCHEMA`.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.summary.schema
    }

    /// What the scan saw: the header, the counts and the window it needed.
    #[must_use]
    pub fn summary(&self) -> &StreamSummary {
        &self.summary
    }

    /// The number of instances.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the file has no instance.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The names of the instances in ascending order.
    pub fn names(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries.keys().copied()
    }

    /// Whether an instance with this name exists.
    #[must_use]
    pub fn contains(&self, name: u64) -> bool {
        self.entries.contains_key(&name)
    }

    /// The byte span of an instance in the physical file, as [`IfcFile::span`](crate::IfcFile::span).
    #[must_use]
    pub fn span(&self, name: u64) -> Option<Range<u64>> {
        self.entries.get(&name).map(|e| e.start..e.end)
    }

    /// The names of the instances with a record of the type, in file order. The type name is
    /// compared in upper case.
    #[must_use]
    pub fn instances_of(&self, type_name: &str) -> &[u64] {
        self.by_type
            .get(&type_name.to_ascii_uppercase())
            .map_or(&[], Vec::as_slice)
    }

    /// The estimate of the memory the index holds.
    #[must_use]
    pub fn estimated_bytes(&self) -> u64 {
        self.estimated_bytes
    }
}

/// An index over a seekable source: instances are parsed on demand from their spans.
pub struct StreamFile<R: Read + Seek> {
    source: R,
    index: StreamIndex,
    limits: Limits,
    cancel: Cancel,
}

impl<R: Read + Seek> StreamFile<R> {
    /// Indexes `source` and keeps it for on-demand reads.
    ///
    /// # Errors
    ///
    /// As [`StreamIndex::build`]; `io.ifc.truncated` when the source cannot seek back.
    pub fn open(
        mut source: R,
        limits: &Limits,
        stream: &StreamLimits,
        cancel: &Cancel,
    ) -> Result<Self> {
        let index = StreamIndex::build(&mut source, limits, stream, cancel)?;
        Ok(Self {
            source,
            index,
            limits: *limits,
            cancel: cancel.clone(),
        })
    }

    /// The index.
    #[must_use]
    pub fn index(&self) -> &StreamIndex {
        &self.index
    }

    /// The instance with the given name, parsed from its span; `None` when there is none.
    ///
    /// # Errors
    ///
    /// `io.ifc.truncated` when the source cannot be read at the span; the parse errors of
    /// [`read`](crate::read) when the source changed since it was indexed.
    pub fn instance(&mut self, name: u64) -> Result<Option<Instance>> {
        let Some(span) = self.index.span(name) else {
            return Ok(None);
        };
        let length = usize::try_from(span.end - span.start)
            .map_err(|_| limit("string_bytes", span.end - span.start))?;
        self.source
            .seek(SeekFrom::Start(span.start))
            .map_err(|_| Error::new(codes::IFC_TRUNCATED))?;
        let mut bytes = vec![0u8; length];
        self.source
            .read_exact(&mut bytes)
            .map_err(|_| Error::new(codes::IFC_TRUNCATED))?;
        let mut control = Control::new(&self.cancel, None, None, None);
        parse_instance_statement(&bytes, &self.limits, &mut control)
            .map(Some)
            .map_err(|e| {
                e.with_detail(
                    "statement_offset",
                    i64::try_from(span.start).unwrap_or(i64::MAX),
                )
            })
    }

    /// The instances with a record of the type, parsed in file order.
    ///
    /// # Errors
    ///
    /// As [`Self::instance`].
    pub fn instances_of(&mut self, type_name: &str) -> Result<Vec<Instance>> {
        let names: Vec<u64> = self.index.instances_of(type_name).to_vec();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            self.cancel.check()?;
            if let Some(instance) = self.instance(name)? {
                out.push(instance);
            }
        }
        Ok(out)
    }
}
