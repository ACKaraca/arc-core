//! A bounded pull reader for XML documents in a byte buffer, built on `quick-xml`.
//!
//! The reader passes on three kinds of event: a start element with its name and attributes, an
//! end element and text. Comments and the XML declaration are consumed silently. It refuses a
//! document type declaration, a processing instruction, an entity reference other than the five
//! predefined entities and character references, and a declared encoding other than UTF-8, so
//! entity expansion and external entities cannot occur. Names, attribute values and text are
//! UTF-8, and line ends are normalized as XML requires.
//!
//! Namespace declarations stay as ordinary attributes named `xmlns` or `xmlns:prefix`. The reader
//! keeps the declarations in scope, and [`XmlReader::resolve_prefix`] resolves a prefix against
//! them.
//!
//! Limits (see [`arc_base::Limits`]): the input size; the element depth (`nesting_depth`); the
//! number of attributes per element and of in-scope namespace declarations (`array_length`); the
//! bytes of an element's attributes, a name and a text node (`string_bytes`); the number of parser
//! events (`record_count`). The cancellation token is checked every 1024 events.

mod text;

use std::collections::VecDeque;

use arc_base::{Cancel, Error, Limits, Result};
use quick_xml::errors::{Error as QuickError, IllFormedError, SyntaxError};
use quick_xml::events::attributes::AttrError;
use quick_xml::events::{BytesDecl, BytesStart, Event as QuickEvent};

use crate::bytes::to_u64;
use crate::error::{codes, limit};
use crate::options::ReadOptions;
use text::Mode;

/// The namespace name bound to the prefix `xml`.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The number of parser events between two cancellation checks.
const CHECK_INTERVAL: u64 = 1024;

/// An attribute of a start element, with its value decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// The qualified name, such as `xmlns:a` or `a:name`.
    pub name: String,
    /// The value after entity decoding and attribute-value normalization.
    pub value: String,
}

/// A start element: its qualified name and its attributes in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The qualified name.
    pub name: String,
    /// The attributes, without the namespace declarations' special meaning.
    pub attributes: Vec<Attribute>,
}

/// An event of the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The start of an element. An empty element `<a/>` yields a start and an end.
    Start(Element),
    /// The end of the element with this qualified name.
    End(String),
    /// Character data between markup. Adjacent character data, CDATA sections and references
    /// form one text node.
    Text(String),
}

/// Splits a qualified name into its prefix (empty when there is none) and its local name.
#[must_use]
pub fn split_qualified_name(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("", name))
}

/// A namespace declaration in scope: a prefix (empty for the default namespace) and its URI.
struct Declaration {
    prefix: String,
    uri: String,
}

/// An element that is open, with the number of declarations in scope before it.
struct Open {
    name: String,
    scope_len: usize,
}

/// A pull reader over an XML document in a byte buffer.
pub struct XmlReader<'a> {
    reader: quick_xml::Reader<&'a [u8]>,
    limits: Limits,
    cancel: Cancel,
    open: Vec<Open>,
    scope: Vec<Declaration>,
    text: Vec<u8>,
    queue: VecDeque<Event>,
    events: u64,
    root_seen: bool,
    done: bool,
}

impl<'a> XmlReader<'a> {
    /// A reader over `data` under the limits and token of `options`.
    ///
    /// A leading UTF-8 byte-order mark is skipped.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when the input is larger than `Limits::input_bytes`;
    /// `io.xml.encoding_unsupported` for a UTF-16 byte-order mark.
    pub fn new(data: &'a [u8], options: &ReadOptions) -> Result<Self> {
        let length = to_u64(data.len());
        if length > options.limits.input_bytes {
            return Err(limit("input_bytes", length));
        }
        let data = if data.starts_with(&[0xFE, 0xFF]) || data.starts_with(&[0xFF, 0xFE]) {
            return Err(Error::new(codes::XML_ENCODING_UNSUPPORTED));
        } else if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
            data.get(3..).unwrap_or_default()
        } else {
            data
        };
        Ok(Self {
            reader: quick_xml::Reader::from_reader(data),
            limits: options.limits,
            cancel: options.cancel.clone(),
            open: Vec::new(),
            scope: Vec::new(),
            text: Vec::new(),
            queue: VecDeque::new(),
            events: 0,
            root_seen: false,
            done: false,
        })
    }

    /// The next event, or `None` at the end of the document.
    ///
    /// # Errors
    ///
    /// The `io.xml.*` code of the first violation; `io.limit_exceeded` for a limit;
    /// `base.cancelled` when the token was cancelled.
    pub fn next_event(&mut self) -> Result<Option<Event>> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Ok(Some(event));
            }
            if self.done {
                return Ok(None);
            }
            match self.read_raw()? {
                QuickEvent::Text(raw) => {
                    let bytes: &[u8] = &raw;
                    if bytes.windows(3).any(|window| window == b"]]>") {
                        return Err(Error::new(codes::XML_SYNTAX));
                    }
                    self.append(bytes)?;
                }
                QuickEvent::GeneralRef(raw) => {
                    self.outside_root_refused()?;
                    self.append(b"&")?;
                    self.append(&raw)?;
                    self.append(b";")?;
                }
                QuickEvent::CData(raw) => {
                    self.outside_root_refused()?;
                    for &byte in raw.iter() {
                        if byte == b'&' {
                            self.append(b"&amp;")?;
                        } else {
                            self.append(&[byte])?;
                        }
                    }
                }
                other => {
                    if let Some(text) = self.take_text()? {
                        self.queue.push_back(text);
                    }
                    self.markup(other)?;
                }
            }
        }
    }

    /// The URI bound to `prefix` by the declarations in scope; the empty prefix is the default
    /// namespace. `xml` is bound to [`XML_NAMESPACE`]. `None` when the prefix is not bound.
    #[must_use]
    pub fn resolve_prefix(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.scope
            .iter()
            .rev()
            .find(|declaration| declaration.prefix == prefix)
            .map(|declaration| declaration.uri.as_str())
    }

    fn read_raw(&mut self) -> Result<QuickEvent<'a>> {
        self.events = self.events.saturating_add(1);
        if self.events > self.limits.record_count {
            return Err(limit("record_count", self.events));
        }
        if self.events.is_multiple_of(CHECK_INTERVAL) {
            self.cancel.check()?;
        }
        self.reader.read_event().map_err(map_error)
    }

    /// Character data, references and CDATA sections are refused outside the root element; only
    /// white space may stand there.
    fn outside_root_refused(&self) -> Result<()> {
        if self.open.is_empty() {
            return Err(Error::new(codes::XML_STRUCTURE_INVALID));
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<()> {
        let total = to_u64(self.text.len().saturating_add(bytes.len()));
        if total > self.limits.string_bytes {
            return Err(limit("string_bytes", total));
        }
        self.text.extend_from_slice(bytes);
        Ok(())
    }

    /// The pending text node as an event, or nothing when it is only white space outside the root.
    fn take_text(&mut self) -> Result<Option<Event>> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let raw = std::mem::take(&mut self.text);
        let decoded = text::decode(&raw, Mode::Text)?;
        if self.open.is_empty() {
            return if decoded
                .chars()
                .all(|ch| matches!(ch, ' ' | '\t' | '\r' | '\n'))
            {
                Ok(None)
            } else {
                Err(Error::new(codes::XML_STRUCTURE_INVALID))
            };
        }
        Ok(Some(Event::Text(decoded)))
    }

    fn markup(&mut self, event: QuickEvent<'a>) -> Result<()> {
        match event {
            QuickEvent::Start(start) => self.start(&start, false),
            QuickEvent::Empty(start) => self.start(&start, true),
            QuickEvent::End(end) => {
                let name = qualified_name(end.name().as_ref())?;
                self.end_element(name)
            }
            // The declaration is accepted only as the first event; anywhere else it is a
            // processing instruction, which is refused.
            QuickEvent::Decl(decl) if self.events == 1 => check_declaration(&decl),
            QuickEvent::Decl(_) => Err(Error::new(codes::XML_PROCESSING_INSTRUCTION_REFUSED)),
            QuickEvent::PI(_) => Err(Error::new(codes::XML_PROCESSING_INSTRUCTION_REFUSED)),
            QuickEvent::DocType(_) => Err(Error::new(codes::XML_DOCTYPE_REFUSED)),
            QuickEvent::Eof => self.finish(),
            _ => Ok(()),
        }
    }

    fn start(&mut self, start: &BytesStart<'_>, empty: bool) -> Result<()> {
        if self.open.is_empty() && self.root_seen {
            return Err(Error::new(codes::XML_STRUCTURE_INVALID));
        }
        let name = qualified_name(start.name().as_ref())?;
        self.check_length(name.len())?;
        if to_u64(self.open.len()) >= u64::from(self.limits.nesting_depth) {
            return Err(limit("nesting_depth", to_u64(self.open.len()) + 1));
        }
        let attributes = self.attributes(start)?;
        let scope_len = self.scope.len();
        for attribute in &attributes {
            if attribute.name == "xmlns" {
                self.declare(String::new(), &attribute.value)?;
            } else if let Some(prefix) = attribute.name.strip_prefix("xmlns:") {
                // An empty prefix would bind the default namespace; qualified_name refuses it.
                if prefix.is_empty() {
                    return Err(Error::new(codes::XML_STRUCTURE_INVALID));
                }
                self.declare(prefix.to_owned(), &attribute.value)?;
            }
        }
        self.root_seen = true;
        self.open.push(Open {
            name: name.clone(),
            scope_len,
        });
        self.queue.push_back(Event::Start(Element {
            name: name.clone(),
            attributes,
        }));
        if empty {
            self.end_element(name)?;
        }
        Ok(())
    }

    fn attributes(&self, start: &BytesStart<'_>) -> Result<Vec<Attribute>> {
        let mut attributes = Vec::new();
        let mut bytes: u64 = 0;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(map_attribute_error)?;
            if to_u64(attributes.len()) >= self.limits.array_length {
                return Err(limit("array_length", to_u64(attributes.len()) + 1));
            }
            let name = qualified_name(attribute.key.as_ref())?;
            let value = text::decode(&attribute.value, Mode::Attribute)?;
            bytes = bytes.saturating_add(to_u64(name.len() + attribute.value.len()));
            if bytes > self.limits.string_bytes {
                return Err(limit("string_bytes", bytes));
            }
            attributes.push(Attribute { name, value });
        }
        Ok(attributes)
    }

    fn declare(&mut self, prefix: String, uri: &str) -> Result<()> {
        if to_u64(self.scope.len() + 1) > self.limits.array_length {
            return Err(limit("array_length", to_u64(self.scope.len() + 1)));
        }
        self.scope.push(Declaration {
            prefix,
            uri: uri.to_owned(),
        });
        Ok(())
    }

    fn check_length(&self, length: usize) -> Result<()> {
        let length = to_u64(length);
        if length > self.limits.string_bytes {
            return Err(limit("string_bytes", length));
        }
        Ok(())
    }

    fn end_element(&mut self, name: String) -> Result<()> {
        let open = self
            .open
            .pop()
            .ok_or_else(|| Error::new(codes::XML_STRUCTURE_INVALID))?;
        if open.name != name {
            return Err(Error::new(codes::XML_STRUCTURE_INVALID));
        }
        self.scope.truncate(open.scope_len);
        self.queue.push_back(Event::End(name));
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if !self.root_seen || !self.open.is_empty() {
            return Err(Error::new(codes::XML_TRUNCATED));
        }
        self.done = true;
        Ok(())
    }
}

fn check_declaration(decl: &BytesDecl<'_>) -> Result<()> {
    if let Some(encoding) = decl.encoding() {
        let encoding = encoding.map_err(|_| Error::new(codes::XML_SYNTAX))?;
        let encoding = encoding.as_ref();
        if !(encoding.eq_ignore_ascii_case(b"utf-8") || encoding.eq_ignore_ascii_case(b"us-ascii"))
        {
            return Err(Error::new(codes::XML_ENCODING_UNSUPPORTED));
        }
    }
    Ok(())
}

fn utf8(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| Error::new(codes::XML_TEXT_INVALID))
}

/// A qualified name: UTF-8, a `Name` of the XML grammar, and at most one colon that is neither
/// first nor last. The grammar is a syntax error; a misplaced colon is a structure error.
fn qualified_name(bytes: &[u8]) -> Result<String> {
    let name = utf8(bytes)?;
    if !text::is_name(&name) {
        return Err(Error::new(codes::XML_SYNTAX));
    }
    let colons = name.matches(':').count();
    let placed = name.starts_with(':') || name.ends_with(':');
    if colons > 1 || (colons == 1 && placed) {
        return Err(Error::new(codes::XML_STRUCTURE_INVALID));
    }
    Ok(name)
}

fn map_attribute_error(error: AttrError) -> Error {
    match error {
        AttrError::Duplicated(..) => Error::new(codes::XML_STRUCTURE_INVALID),
        _ => Error::new(codes::XML_SYNTAX),
    }
}

/// Maps a `quick-xml` error to the code of the matching violation.
fn map_error(error: QuickError) -> Error {
    match error {
        QuickError::Syntax(
            SyntaxError::UnclosedTag
            | SyntaxError::UnclosedPI
            | SyntaxError::UnclosedXmlDecl
            | SyntaxError::UnclosedComment
            | SyntaxError::UnclosedDoctype
            | SyntaxError::UnclosedCData
            | SyntaxError::UnclosedSingleQuotedAttributeValue
            | SyntaxError::UnclosedDoubleQuotedAttributeValue,
        )
        | QuickError::IllFormed(IllFormedError::MissingEndTag(_)) => {
            Error::new(codes::XML_TRUNCATED)
        }
        QuickError::IllFormed(_) => Error::new(codes::XML_STRUCTURE_INVALID),
        QuickError::Escape(_) => Error::new(codes::XML_REFERENCE_INVALID),
        QuickError::Encoding(_) => Error::new(codes::XML_TEXT_INVALID),
        _ => Error::new(codes::XML_SYNTAX),
    }
}
