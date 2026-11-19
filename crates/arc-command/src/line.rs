//! Command-line parsing (CORE-CMD-013): the grammar of a typed line, the conversion of inline
//! values to parameter JSON by the parameter's schema kind, the canonical line form
//! ([`format_line`]) and completion data ([`complete`]).
//!
//! The grammar is specified in EBNF in the crate specification. Every error carries a stable
//! code and the byte span of the offending text in the details `start` and `end`.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Dimension, Error, Result, Unit};
use arc_doc::Snapshot;
use serde_json::{Map, Value};

use crate::error::codes;
use crate::invoke::Invocation;
use crate::registry::{Registry, Via};
use crate::schema;
use crate::spec::{CommandSpec, Surface};

/// The longest line, in bytes.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// The most arguments of one line.
pub const MAX_ARGUMENTS: usize = 1024;

/// The deepest nesting of lists.
pub const MAX_LIST_DEPTH: usize = 32;

/// The deepest nesting of parentheses in a bare value (as `arc_solve::ExprLimits::MAX_DEPTH`).
pub const MAX_GROUP_DEPTH: usize = 128;

/// The most completion candidates returned.
pub const MAX_CANDIDATES: usize = 100;

/// The most objects of a snapshot that completion looks at.
pub const MAX_COMPLETION_SCAN: usize = 100_000;

/// A byte range of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The first byte.
    pub start: usize,
    /// The byte after the last.
    pub end: usize,
}

fn at(error: Error, span: Span) -> Error {
    error
        .with_detail("start", i64::try_from(span.start).unwrap_or(i64::MAX))
        .with_detail("end", i64::try_from(span.end).unwrap_or(i64::MAX))
}

fn syntax(reason: &'static str, start: usize, end: usize) -> Error {
    at(
        Error::new(codes::LINE_SYNTAX).with_detail("reason", reason),
        Span { start, end },
    )
}

fn limit(name: &'static str, start: usize, end: usize) -> Error {
    at(
        Error::new(codes::LINE_LIMIT_EXCEEDED).with_detail("limit", name),
        Span { start, end },
    )
}

/// A value as typed.
#[derive(Debug, Clone, PartialEq)]
enum Raw {
    /// Unquoted text, parentheses included.
    Bare(String),
    /// Quoted text with escapes resolved.
    Quoted(String),
    /// `[v, ...]`.
    List(Vec<Raw>),
    /// `$` followed by a JSON value.
    Json(Value),
}

#[derive(Debug)]
struct Argument {
    name: Option<String>,
    value: Raw,
    span: Span,
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.text
            .get(self.pos..)
            .and_then(|rest| rest.chars().next())
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        self.pos > start
    }

    /// `true` when the next character ends a value in this context.
    fn at_end_of_value(&self, in_list: bool) -> bool {
        match self.peek() {
            None => true,
            Some(c) => c.is_whitespace() || (in_list && matches!(c, ',' | ']')),
        }
    }

    fn quoted(&mut self) -> Result<String> {
        let start = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(syntax("unterminated_quote", start, self.pos)),
                Some('"') => return Ok(out),
                Some('\\') => {
                    let escape = self.pos - 1;
                    match self.bump() {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        _ => return Err(syntax("escape", escape, self.pos)),
                    }
                }
                Some(c) => out.push(c),
            }
        }
    }

    fn bare(&mut self, in_list: bool) -> Result<String> {
        let start = self.pos;
        let mut depth = 0_usize;
        while let Some(c) = self.peek() {
            if depth == 0 && (c.is_whitespace() || (in_list && matches!(c, ',' | ']'))) {
                break;
            }
            match c {
                '(' => {
                    depth += 1;
                    if depth > MAX_GROUP_DEPTH {
                        return Err(limit("group_depth", self.pos, self.pos + 1));
                    }
                }
                ')' if depth == 0 => {
                    return Err(syntax("unbalanced_parenthesis", self.pos, self.pos + 1));
                }
                ')' => depth -= 1,
                '"' | '[' | ']' if depth == 0 => {
                    return Err(syntax("character_in_bare_value", self.pos, self.pos + 1));
                }
                _ => {}
            }
            self.bump();
        }
        if depth > 0 {
            return Err(syntax("unterminated_group", start, self.pos));
        }
        Ok(self
            .text
            .get(start..self.pos)
            .unwrap_or_default()
            .to_owned())
    }

    fn json(&mut self) -> Result<Value> {
        let start = self.pos;
        self.bump();
        let rest = self.text.get(self.pos..).unwrap_or_default();
        if rest.starts_with(char::is_whitespace) {
            return Err(syntax("json", start, self.pos));
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                self.pos += stream.byte_offset();
                Ok(value)
            }
            _ => Err(syntax("json", start, self.text.len())),
        }
    }

    fn value(&mut self, in_list: bool, depth: usize) -> Result<Raw> {
        let start = self.pos;
        let raw = match self.peek() {
            Some('"') => Raw::Quoted(self.quoted()?),
            Some('$') => Raw::Json(self.json()?),
            Some('[') => {
                if depth >= MAX_LIST_DEPTH {
                    return Err(limit("list_depth", start, start + 1));
                }
                self.bump();
                let mut items = Vec::new();
                self.skip_ws();
                if self.peek() == Some(']') {
                    self.bump();
                } else {
                    loop {
                        if items.len() >= MAX_ARGUMENTS {
                            return Err(limit("list_length", start, self.pos));
                        }
                        items.push(self.value(true, depth + 1)?);
                        self.skip_ws();
                        match self.bump() {
                            Some(',') => {
                                self.skip_ws();
                            }
                            Some(']') => break,
                            None => return Err(syntax("unterminated_list", start, self.pos)),
                            Some(_) => {
                                return Err(syntax("list_separator", self.pos - 1, self.pos));
                            }
                        }
                    }
                }
                Raw::List(items)
            }
            Some(c) if c.is_whitespace() || (in_list && matches!(c, ',' | ']')) || c == ']' => {
                return Err(syntax("missing_value", start, start));
            }
            None => return Err(syntax("missing_value", start, start)),
            Some(_) => Raw::Bare(self.bare(in_list)?),
        };
        if !self.at_end_of_value(in_list) {
            return Err(syntax("after_value", self.pos, self.pos));
        }
        Ok(raw)
    }

    fn argument(&mut self) -> Result<Argument> {
        let start = self.pos;
        let mut name = None;
        if self.peek().is_some_and(is_name_start) {
            let mut end = self.pos;
            let rest = self.text.get(self.pos..).unwrap_or_default();
            for c in rest.chars() {
                if !is_name_char(c) {
                    break;
                }
                end += c.len_utf8();
            }
            if self.text.get(end..).is_some_and(|r| r.starts_with('=')) {
                name = Some(self.text.get(start..end).unwrap_or_default().to_owned());
                self.pos = end + 1;
            }
        }
        let value = self.value(false, 0)?;
        Ok(Argument {
            name,
            value,
            span: Span {
                start,
                end: self.pos,
            },
        })
    }
}

/// The command name and arguments of a line, or `None` for a blank line.
fn tokenize(text: &str) -> Result<Option<(String, Span, Vec<Argument>)>> {
    if text.len() > MAX_LINE_BYTES {
        return Err(limit("line_bytes", 0, text.len()));
    }
    let mut lexer = Lexer { text, pos: 0 };
    lexer.skip_ws();
    let start = lexer.pos;
    let name = match lexer.peek() {
        None => return Ok(None),
        Some('"') => {
            let name = lexer.quoted()?;
            if !lexer.at_end_of_value(false) {
                return Err(syntax("after_value", lexer.pos, lexer.pos));
            }
            name
        }
        Some(_) => {
            while lexer.peek().is_some_and(|c| !c.is_whitespace()) {
                lexer.bump();
            }
            text.get(start..lexer.pos).unwrap_or_default().to_owned()
        }
    };
    let name_span = Span {
        start,
        end: lexer.pos,
    };
    let mut arguments = Vec::new();
    loop {
        lexer.skip_ws();
        if lexer.peek().is_none() {
            break;
        }
        if arguments.len() >= MAX_ARGUMENTS {
            return Err(limit("arguments", lexer.pos, text.len()));
        }
        arguments.push(lexer.argument()?);
    }
    Ok(Some((name, name_span, arguments)))
}

/// How an inline value converts to a parameter's JSON, by the parameter's schema.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// `anyOf` number and string: a quantity, kept as text.
    Quantity,
    /// `number`.
    Number,
    /// `integer`.
    Integer,
    /// `boolean`.
    Boolean,
    /// `string` with `format: uuid`.
    Id,
    /// Other strings.
    Text,
    /// A closed list of values (`enum`, or `oneOf` of `const`).
    Enum(Vec<Value>),
    /// `array` with `items`.
    Array(Box<Kind>),
    /// Anything else: only `$` JSON and plain text.
    Json,
}

/// The depth of `$ref` and `anyOf` chains followed when classifying a schema.
const MAX_KIND_DEPTH: u32 = 16;

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let mut schema = schema;
    for _ in 0..MAX_KIND_DEPTH {
        let Some(name) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
        else {
            break;
        };
        match root.get("$defs").and_then(|d| d.get(name)) {
            Some(target) => schema = target,
            None => break,
        }
    }
    schema
}

fn is_null(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

fn kind_at(root: &Value, schema: &Value, depth: u32) -> Kind {
    if depth > MAX_KIND_DEPTH {
        return Kind::Json;
    }
    let schema = resolve(root, schema);
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Kind::Enum(values.iter().filter(|v| !v.is_null()).cloned().collect());
    }
    for key in ["anyOf", "oneOf"] {
        let Some(branches) = schema.get(key).and_then(Value::as_array) else {
            continue;
        };
        let branches: Vec<&Value> = branches
            .iter()
            .map(|b| resolve(root, b))
            .filter(|b| !is_null(b))
            .collect();
        let types: BTreeSet<&str> = branches
            .iter()
            .filter_map(|b| b.get("type").and_then(Value::as_str))
            .collect();
        if branches.len() == 2 && types == BTreeSet::from(["number", "string"]) {
            return Kind::Quantity;
        }
        if !branches.is_empty() && branches.iter().all(|b| b.get("const").is_some()) {
            return Kind::Enum(
                branches
                    .iter()
                    .filter_map(|b| b.get("const"))
                    .cloned()
                    .collect(),
            );
        }
        if let [only] = branches.as_slice() {
            return kind_at(root, only, depth + 1);
        }
        return Kind::Json;
    }
    if let Some(value) = schema.get("const") {
        return Kind::Enum(vec![value.clone()]);
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(types)) => {
            let types: Vec<&str> = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| *t != "null")
                .collect();
            match types.as_slice() {
                [one] => Some(*one),
                _ => None,
            }
        }
        _ => None,
    };
    match ty {
        Some("number") => Kind::Number,
        Some("integer") => Kind::Integer,
        Some("boolean") => Kind::Boolean,
        Some("string") if schema.get("format").and_then(Value::as_str) == Some("uuid") => Kind::Id,
        Some("string") => Kind::Text,
        Some("array") => match schema.get("items") {
            Some(items) if items.is_object() => {
                Kind::Array(Box::new(kind_at(root, items, depth + 1)))
            }
            _ => Kind::Json,
        },
        _ => Kind::Json,
    }
}

/// The kind of the parameter `name` of the parameter schema `root`.
pub(crate) fn param_kind(root: &Value, name: &str) -> Kind {
    root.get("properties")
        .and_then(|p| p.get(name))
        .map_or(Kind::Json, |schema| kind_at(root, schema, 0))
}

/// Splits `text` at `separator` outside parentheses; parts are trimmed.
pub(crate) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c == separator && depth == 0 => {
                parts.push(text.get(start..i).unwrap_or_default().trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default().trim());
    parts
}

/// Converts text (bare or quoted) to JSON by `kind`; `None` when it does not fit.
fn text_to(kind: &Kind, text: &str, quoted: bool) -> Option<Value> {
    match kind {
        Kind::Quantity | Kind::Text | Kind::Id | Kind::Json => Some(Value::String(text.to_owned())),
        Kind::Number | Kind::Integer => {
            let value: Value = serde_json::from_str(text).ok()?;
            let fits = match &value {
                Value::Number(n) => *kind == Kind::Number || n.is_i64() || n.is_u64(),
                _ => false,
            };
            fits.then_some(value)
        }
        Kind::Boolean => match text {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        Kind::Enum(values) => values
            .iter()
            .find(|v| v.as_str() == Some(text))
            .or_else(|| {
                values.iter().find(|v| {
                    !v.is_string() && serde_json::to_string(v).ok().as_deref() == Some(text)
                })
            })
            .cloned(),
        Kind::Array(item) if !quoted => split_top_level(text, ',')
            .into_iter()
            .map(|part| {
                (!part.is_empty())
                    .then(|| text_to(item, part, false))
                    .flatten()
            })
            .collect::<Option<Vec<Value>>>()
            .map(Value::Array),
        Kind::Array(_) => None,
    }
}

fn convert(raw: &Raw, kind: &Kind) -> Option<Value> {
    match raw {
        Raw::Json(value) => Some(value.clone()),
        Raw::Bare(text) => text_to(kind, text, false),
        Raw::Quoted(text) => text_to(kind, text, true),
        Raw::List(items) => {
            let item = match kind {
                Kind::Array(item) => item.as_ref(),
                Kind::Json => &Kind::Json,
                _ => return None,
            };
            items
                .iter()
                .map(|raw| convert(raw, item))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        }
    }
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedLine {
    /// The command identifier.
    pub command: String,
    /// How the typed name was resolved.
    pub via: Via,
    /// The span of the typed name.
    pub name_span: Span,
    /// The inline parameters.
    pub params: Map<String, Value>,
    /// The span of each inline parameter's argument.
    pub spans: BTreeMap<String, Span>,
    /// The required parameters still missing, in the order of `required`.
    pub missing: Vec<String>,
}

/// Checks inline parameters against the parameter schema without its `required` list; the
/// first violation is reported with the span of its argument.
fn check_partial(
    root: &Value,
    value: &Value,
    spans: &BTreeMap<String, Span>,
    first_span: Span,
) -> Result<()> {
    let mut partial = root.clone();
    if let Some(object) = partial.as_object_mut() {
        object.remove("required");
    }
    let violations = schema::validate(&partial, value)?;
    if let Some(first) = violations.first() {
        let param = first
            .pointer
            .split('/')
            .nth(1)
            .unwrap_or_default()
            .replace("~1", "/")
            .replace("~0", "~");
        let span = spans.get(&param).copied().unwrap_or(first_span);
        return Err(at(
            Error::new(codes::LINE_VALUE)
                .with_arg("name", param)
                .with_detail(
                    "violations",
                    serde_json::to_string(&violations).unwrap_or_default(),
                ),
            span,
        ));
    }
    Ok(())
}

/// Parses a typed line: the command name, resolved through [`Registry::resolve`] with the
/// `localized` names, then positional and named inline parameters converted by each parameter's
/// schema kind and checked against the parameter schema without its `required` list. Returns
/// `Ok(None)` for a blank line.
///
/// # Errors
///
/// `cmd.line.syntax`, `cmd.line.limit_exceeded`, `cmd.unknown`, `cmd.name.ambiguous`,
/// `cmd.line.unknown_param`, `cmd.line.duplicate_param`, `cmd.line.too_many_values`,
/// `cmd.line.value` and `cmd.params.limit_exceeded`, each with the span of the offending text.
pub fn parse_line(
    registry: &Registry,
    localized: &BTreeMap<String, String>,
    text: &str,
) -> Result<Option<ParsedLine>> {
    let Some((name, name_span, arguments)) = tokenize(text)? else {
        return Ok(None);
    };
    let resolved = registry
        .resolve(&name, localized)
        .map_err(|e| at(e, name_span))?;
    let spec = resolved.spec;
    let root = spec.params_schema();
    let properties = root.get("properties").and_then(Value::as_object);
    let mut params = Map::new();
    let mut spans = BTreeMap::new();
    let mut positional = spec.positional().iter();
    let mut named_seen = false;
    for argument in &arguments {
        let param = match &argument.name {
            Some(name) => {
                named_seen = true;
                if !properties.is_some_and(|p| p.contains_key(name)) {
                    return Err(at(
                        Error::new(codes::LINE_UNKNOWN_PARAM).with_arg("name", name.as_str()),
                        argument.span,
                    ));
                }
                name.clone()
            }
            None if named_seen => {
                return Err(syntax(
                    "positional_after_named",
                    argument.span.start,
                    argument.span.end,
                ));
            }
            None => match positional.next() {
                Some(name) => name.clone(),
                None => return Err(at(Error::new(codes::LINE_TOO_MANY_VALUES), argument.span)),
            },
        };
        if params.contains_key(&param) {
            return Err(at(
                Error::new(codes::LINE_DUPLICATE_PARAM).with_arg("name", param.as_str()),
                argument.span,
            ));
        }
        let kind = param_kind(root, &param);
        let value = convert(&argument.value, &kind).ok_or_else(|| {
            at(
                Error::new(codes::LINE_VALUE).with_arg("name", param.as_str()),
                argument.span,
            )
        })?;
        params.insert(param.clone(), value);
        spans.insert(param, argument.span);
    }
    let value = Value::Object(params);
    let first_span = arguments.first().map_or(name_span, |a| a.span);
    schema::check_limits(&value, &arc_base::Limits::DEFAULT).map_err(|e| at(e, first_span))?;
    check_partial(root, &value, &spans, first_span)?;
    let Value::Object(params) = value else {
        return Err(Error::new(codes::LINE_VALUE));
    };
    let missing = root
        .get("required")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| !params.contains_key(*name))
        .map(str::to_owned)
        .collect();
    Ok(Some(ParsedLine {
        command: spec.id().to_owned(),
        via: resolved.via,
        name_span,
        params,
        spans,
        missing,
    }))
}

fn is_bare_safe(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('$')
        && !text.contains(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '[' | ']' | '(' | ')' | ',' | '\\')
        })
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Reads `text` as one value and converts it by `kind`.
fn read_value(text: &str, kind: &Kind) -> Option<Value> {
    let mut lexer = Lexer { text, pos: 0 };
    let raw = lexer.value(false, 0).ok()?;
    (lexer.pos == text.len())
        .then(|| convert(&raw, kind))
        .flatten()
}

fn format_value(value: &Value, kind: &Kind, depth: usize) -> String {
    let candidate = match (value, kind) {
        (Value::String(s), _) if is_bare_safe(s) => Some(s.clone()),
        (Value::String(s), _) => Some(quote(s)),
        (Value::Number(_) | Value::Bool(_), _) => Some(value.to_string()),
        (Value::Array(items), Kind::Array(item)) if depth < MAX_LIST_DEPTH - 1 => {
            let items: Vec<String> = items
                .iter()
                .map(|v| format_value(v, item, depth + 1))
                .collect();
            Some(format!("[{}]", items.join(", ")))
        }
        _ => None,
    };
    candidate
        .filter(|text| read_value(text, kind).as_ref() == Some(value))
        .unwrap_or_else(|| format!("${}", serde_json::to_string(value).unwrap_or_default()))
}

/// The canonical line of an invocation, which [`parse_line`] reads back to the same command and
/// parameters: the command identifier, then every parameter as `name=value` in name order; a
/// value is written bare when that reads back to the same JSON, else quoted, else as `$` and
/// compact JSON. This is the form macros record.
///
/// # Errors
///
/// `cmd.unknown` for an unknown command, `cmd.line.value` when the parameters are not an
/// object.
pub fn format_line(registry: &Registry, invocation: &Invocation) -> Result<String> {
    let resolved = registry.resolve(&invocation.command, &BTreeMap::new())?;
    let spec = resolved.spec;
    let Value::Object(params) = &invocation.params else {
        return Err(Error::new(codes::LINE_VALUE));
    };
    let mut line = spec.id().to_owned();
    let mut names: Vec<&String> = params.keys().collect();
    names.sort();
    for name in names {
        let value = params.get(name).unwrap_or(&Value::Null);
        let kind = param_kind(spec.params_schema(), name);
        line.push(' ');
        line.push_str(name);
        line.push('=');
        line.push_str(&format_value(value, &kind, 0));
    }
    Ok(line)
}

/// What a completion candidate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// A command identifier, user alias or localized name.
    Command,
    /// A parameter name, written with its `=`.
    Parameter,
    /// A value of a parameter with a closed list of values (keyword options).
    Keyword,
    /// An object whose name starts with the typed text; the text is its identifier and the
    /// candidate's label its name.
    Object,
    /// A unit symbol completing the unit typed after a number.
    Unit,
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The text that replaces the completed range.
    pub text: String,
    /// What it is.
    pub kind: CandidateKind,
    /// A name to show beside the text: the name of an object candidate.
    pub label: Option<String>,
}

/// Completion data: the byte range to replace and the candidates, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    /// The range of the line the candidates replace.
    pub span: Option<Span>,
    /// The candidates, at most [`MAX_CANDIDATES`].
    pub candidates: Vec<Candidate>,
}

fn candidates(
    words: impl IntoIterator<Item = String>,
    partial: &str,
    kind: CandidateKind,
) -> Vec<Candidate> {
    let partial = partial.to_lowercase();
    let set: BTreeSet<String> = words
        .into_iter()
        .filter(|w| w.to_lowercase().starts_with(&partial))
        .collect();
    set.into_iter()
        .take(MAX_CANDIDATES)
        .map(|text| Candidate {
            text,
            kind,
            label: None,
        })
        .collect()
}

fn enum_words(kind: &Kind) -> Vec<String> {
    match kind {
        Kind::Enum(values) => values
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .collect(),
        Kind::Boolean => vec!["false".into(), "true".into()],
        _ => Vec::new(),
    }
}

/// Live objects whose name starts with `partial` (ignoring case), of the listed type keys (any
/// when empty), sorted by name; the text of a candidate is the identifier.
pub(crate) fn object_candidates(
    snapshot: Option<&Snapshot>,
    partial: &str,
    types: &[String],
) -> Vec<Candidate> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let partial = partial.to_lowercase();
    let mut found: Vec<(String, String)> = snapshot
        .iter()
        .take(MAX_COMPLETION_SCAN)
        .filter(|(handle, _)| {
            types.is_empty()
                || snapshot
                    .type_key(*handle)
                    .is_some_and(|key| types.iter().any(|t| t == key.as_str()))
        })
        .filter_map(|(_, object)| {
            let name = object.header.name.as_ref()?;
            name.to_lowercase()
                .starts_with(&partial)
                .then(|| (name.clone(), object.id.to_string()))
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(name, id)| Candidate {
            text: id,
            kind: CandidateKind::Object,
            label: Some(name),
        })
        .collect()
}

fn is_unit_char(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '°' | '²' | '³' | '_')
}

/// Unit symbols completing the letters at the end of `value` when a digit precedes them
/// (`1200m`); of `dimension` when given. Returns the byte offset in `value` where the
/// replaced letters start, and the candidates.
pub(crate) fn unit_candidates(
    value: &str,
    dimension: Option<Dimension>,
) -> (usize, Vec<Candidate>) {
    let letters: usize = value
        .chars()
        .rev()
        .take_while(|c| is_unit_char(*c))
        .map(char::len_utf8)
        .sum();
    let start = value.len() - letters;
    let (before, run) = value.split_at(start);
    if run.is_empty() || !before.ends_with(|c: char| c.is_ascii_digit()) {
        return (0, Vec::new());
    }
    let symbols = Unit::all()
        .iter()
        .filter(|u| dimension.is_none_or(|d| u.dimension == d))
        .map(|u| u.symbol.to_owned());
    (start, candidates(symbols, run, CandidateKind::Unit))
}

/// The candidates for the value typed so far of a parameter of `kind`, and the byte offset in
/// `value` where the replaced text starts.
fn value_candidates(
    kind: &Kind,
    value: &str,
    snapshot: Option<&Snapshot>,
) -> (usize, Vec<Candidate>) {
    match kind {
        Kind::Enum(_) | Kind::Boolean => (
            0,
            candidates(enum_words(kind), value, CandidateKind::Keyword),
        ),
        Kind::Id => (0, object_candidates(snapshot, value, &[])),
        Kind::Array(inner) if matches!(**inner, Kind::Id) => {
            let start = value.rfind([',', '[']).map_or(0, |i| i + 1);
            (
                start,
                object_candidates(snapshot, value.get(start..).unwrap_or(""), &[]),
            )
        }
        Kind::Quantity => unit_candidates(value, None),
        _ => (0, Vec::new()),
    }
}

/// The cursor `cursor` moved into the line: at most its length and [`MAX_LINE_BYTES`], then back to
/// the start of the character it falls in.
pub(crate) fn clamp_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len()).min(MAX_LINE_BYTES);
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// [`complete_in`] without a document: object names are not completed.
#[must_use]
pub fn complete(
    registry: &Registry,
    localized: &BTreeMap<String, String>,
    text: &str,
    cursor: usize,
) -> Completion {
    complete_in(registry, localized, None, text, cursor)
}

/// Completion for a partial line with the cursor at byte `cursor` (clamped to the line and
/// moved back to a character boundary): command names exposed on the command line in the first
/// word; after it, the names of parameters not yet given (as `name=`) and the values of the
/// parameter being typed when it has a closed list of values. The text before the cursor is
/// split at whitespace, so quoted text with spaces is not completed. Data only.
///
/// A parameter that takes an object identifier completes the names of the objects of `snapshot`
/// (the candidate text is the identifier, the label the name); a quantity parameter completes
/// the unit symbol typed after a number (`1200m`). Both need no more than the text before the
/// cursor.
#[must_use]
pub fn complete_in(
    registry: &Registry,
    localized: &BTreeMap<String, String>,
    snapshot: Option<&Snapshot>,
    text: &str,
    cursor: usize,
) -> Completion {
    let cursor = clamp_cursor(text, cursor);
    let prefix = text.get(..cursor).unwrap_or_default();
    let lead = prefix.len() - prefix.trim_start().len();
    let words: Vec<&str> = prefix.split_whitespace().collect();
    let ends_in_space = prefix.ends_with(char::is_whitespace);
    let partial = if ends_in_space {
        ""
    } else {
        words.last().copied().unwrap_or("")
    };
    let start = cursor - partial.len();
    let span = Some(Span { start, end: cursor });
    if words.is_empty() || (words.len() == 1 && !ends_in_space) {
        let exposed = |id: &str| {
            registry
                .get(id)
                .is_some_and(|spec| spec.exposure().contains(&Surface::CommandLine))
        };
        let names = registry
            .iter()
            .filter(|s| s.exposure().contains(&Surface::CommandLine))
            .map(|s| s.id().to_owned())
            .chain(
                registry
                    .user_aliases()
                    .filter(|(_, id)| exposed(id))
                    .map(|(alias, _)| alias.to_owned()),
            )
            .chain(
                localized
                    .iter()
                    .filter(|(id, _)| exposed(id))
                    .map(|(_, name)| name.clone()),
            );
        return Completion {
            span: Some(Span {
                start: lead,
                end: cursor,
            }),
            candidates: candidates(names, partial, CandidateKind::Command),
        };
    }
    let Some(name) = words.first() else {
        return Completion::default();
    };
    let Ok(resolved) = registry.resolve(name, localized) else {
        return Completion::default();
    };
    let spec: &CommandSpec = &resolved.spec;
    let root = spec.params_schema();
    if let Some((param, value)) = partial.split_once('=') {
        let (offset, found) = value_candidates(&param_kind(root, param), value, snapshot);
        return Completion {
            span: Some(Span {
                start: cursor - value.len() + offset,
                end: cursor,
            }),
            candidates: found,
        };
    }
    let earlier = words
        .get(1..words.len() - usize::from(!ends_in_space))
        .unwrap_or_default();
    let given: BTreeSet<&str> = earlier
        .iter()
        .filter_map(|w| w.split_once('=').map(|(n, _)| n))
        .collect();
    let positional_count = earlier.iter().filter(|w| !w.contains('=')).count();
    let mut out: Vec<Candidate> = Vec::new();
    if given.is_empty()
        && let Some(param) = spec.positional().get(positional_count)
    {
        let (offset, found) = value_candidates(&param_kind(root, param), partial, snapshot);
        if offset > 0 {
            return Completion {
                span: Some(Span {
                    start: start + offset,
                    end: cursor,
                }),
                candidates: found,
            };
        }
        out = found;
    }
    let names = root
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|p| p.keys())
        .filter(|n| !given.contains(n.as_str()))
        .map(|n| format!("{n}="));
    out.extend(candidates(names, partial, CandidateKind::Parameter));
    out.truncate(MAX_CANDIDATES);
    Completion {
        span,
        candidates: out,
    }
}
