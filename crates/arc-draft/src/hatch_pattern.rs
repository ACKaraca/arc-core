//! Hatch patterns and the Arc hatch-pattern file format, version 1 (CORE-DRAFT-020), and the
//! shipped library.
//!
//! A pattern is a list of line families ([`PatternLine`]): each gives the direction of a set of
//! parallel lines, a point on one of them, the shift from one line to the next and an optional
//! dash list. The file format is a line-oriented UTF-8 text format specified in
//! [`docs/architecture/hatch-pattern-format.md`](../../../docs/architecture/hatch-pattern-format.md);
//! [`PatternFile::parse`] is its bounded reader and [`PatternFile::to_text`] writes the canonical
//! form, which the reader reads back to an equal file.
//!
//! [`library`] is the shipped library, an original work of the project. Line generation from a
//! pattern is in [`crate::hatch`].

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result};
use arc_doc::Value;

use crate::error::codes;

/// The first token of a pattern file.
pub const MAGIC: &str = "arc-hatch-patterns";
/// The format version this reader reads and the writer writes.
pub const VERSION: u32 = 1;
/// The most bytes of a file, whatever the limits allow.
pub const MAX_FILE_BYTES: u64 = 4 << 20;
/// The most bytes of one line.
pub const MAX_LINE_BYTES: usize = 1024;
/// The most patterns of a file.
pub const MAX_PATTERNS: usize = 4096;
/// The most line families of a pattern.
pub const MAX_LINES: usize = 64;
/// The most dash elements of a line family.
pub const MAX_DASHES: usize = 32;
/// The most bytes of a pattern name.
pub const MAX_NAME_BYTES: usize = 64;
/// The most bytes of a description or license.
pub const MAX_TEXT_BYTES: usize = 256;
/// Every number of a line family lies within this distance of zero.
pub const MAX_VALUE: f64 = 1_000_000.0;
/// The largest angle in magnitude, in degrees.
pub const MAX_ANGLE: f64 = 360.0;

const BUILTIN_TEXT: &str = include_str!("../patterns/arc-patterns.ahp");

/// One family of parallel lines of a pattern, in pattern units.
///
/// Line `k` of the family passes through `origin + k · (shift · u + spacing · n)`, where `u` is
/// the unit vector at `angle` degrees from the positive x axis and `n` is `u` turned a quarter
/// turn counterclockwise. The dash list starts on every line at that point and runs along `u`.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternLine {
    /// The direction in degrees, counterclockwise from the positive x axis.
    pub angle: f64,
    /// A point on line 0, whose dash list starts there.
    pub origin: Point2,
    /// The shift of each next line along the direction.
    pub shift: f64,
    /// The perpendicular distance from each line to the next; not zero, and negative counts
    /// lines toward the right of the direction.
    pub spacing: f64,
    /// The dash list: a positive length is drawn, a negative one is a gap, zero is a dot. Empty
    /// is a continuous line.
    pub dashes: Vec<f64>,
}

fn line_invalid(reason: &'static str) -> Error {
    Error::new(codes::PATTERN_LINE_INVALID).with_arg("reason", reason)
}

impl PatternLine {
    /// Checks the rules of a line family: finite numbers within [`MAX_VALUE`] (the angle within
    /// [`MAX_ANGLE`]), a spacing that is not zero, at most [`MAX_DASHES`] dashes and, when there
    /// are any, a total dash length greater than zero.
    ///
    /// # Errors
    ///
    /// `draft.pattern.line_invalid` (argument `reason`).
    pub fn validate(&self) -> Result<()> {
        let within = |v: f64| v.is_finite() && v.abs() <= MAX_VALUE;
        if !(self.angle.is_finite() && self.angle.abs() <= MAX_ANGLE) {
            return Err(line_invalid("angle"));
        }
        if ![self.origin.x, self.origin.y, self.shift]
            .into_iter()
            .all(within)
        {
            return Err(line_invalid("range"));
        }
        if !within(self.spacing) || self.spacing == 0.0 {
            return Err(line_invalid("spacing"));
        }
        if self.dashes.len() > MAX_DASHES {
            return Err(line_invalid("dashes"));
        }
        if !self.dashes.iter().copied().all(within) {
            return Err(line_invalid("range"));
        }
        if !self.dashes.is_empty() && self.dashes.iter().map(|d| d.abs()).sum::<f64>() <= 0.0 {
            return Err(line_invalid("period"));
        }
        Ok(())
    }

    /// The length of one repeat of the dash list in pattern units; 0 for a continuous line.
    #[must_use]
    pub fn period(&self) -> f64 {
        self.dashes.iter().map(|d| d.abs()).sum()
    }

    /// The stored record of a document value.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Record(vec![
            Some(Value::F64(self.angle)),
            Some(Value::Point2(self.origin)),
            Some(Value::F64(self.shift)),
            Some(Value::F64(self.spacing)),
            Some(Value::List(
                self.dashes.iter().map(|d| Value::F64(*d)).collect(),
            )),
        ])
    }

    /// The line family of a stored record; `None` when the value is not one.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let number = |i: usize| match f.get(i) {
            Some(Some(Value::F64(x))) => Some(*x),
            _ => None,
        };
        let dashes = match f.get(4) {
            Some(Some(Value::List(items))) => items
                .iter()
                .map(|d| match d {
                    Value::F64(x) => Some(*x),
                    _ => None,
                })
                .collect::<Option<Vec<f64>>>()?,
            _ => Vec::new(),
        };
        Some(Self {
            angle: number(0)?,
            origin: match f.get(1) {
                Some(Some(Value::Point2(p))) => *p,
                _ => return None,
            },
            shift: number(2)?,
            spacing: number(3)?,
            dashes,
        })
    }
}

/// A named hatch pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchPattern {
    /// The name: lowercase ASCII letters, digits and hyphens, starting with a letter, at most
    /// [`MAX_NAME_BYTES`] bytes.
    pub name: String,
    /// A one-line description.
    pub description: Option<String>,
    /// The line families, at least one.
    pub lines: Vec<PatternLine>,
}

/// A parsed pattern file: its license statement and its patterns in name order.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternFile {
    /// The license statement, if any.
    pub license: Option<String>,
    /// The patterns, sorted by name; names are unique.
    pub patterns: Vec<HatchPattern>,
}

fn syntax(line: usize) -> Error {
    Error::new(codes::PATTERN_SYNTAX).with_arg("line", i64::try_from(line).unwrap_or(i64::MAX))
}

fn limit(name: &str) -> Error {
    Error::new(codes::PATTERN_LIMIT_EXCEEDED).with_arg("limit", name)
}

/// A decimal number: an optional minus sign, 1 to 6 digits, optionally a point and 1 to 6
/// digits. A negative zero reads as zero.
fn number(token: &str, line: usize) -> Result<f64> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    let (whole, fraction) = match digits.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (digits, None),
    };
    let ok = |s: &str| (1..=6).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    if !ok(whole) || !fraction.is_none_or(ok) {
        return Err(syntax(line));
    }
    token
        .parse::<f64>()
        .map(|x| x + 0.0)
        .map_err(|_| syntax(line))
}

fn name_ok(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_NAME_BYTES
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn text_ok(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TEXT_BYTES && !text.chars().any(char::is_control)
}

/// The shortest decimal of the file grammar for `value`: at most six fraction digits.
fn write_number(out: &mut String, value: f64) {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    out.push_str(if text == "-0" || text.is_empty() {
        "0"
    } else {
        text
    });
}

impl HatchPattern {
    /// Checks the rules of a pattern: a valid name, at least one and at most [`MAX_LINES`]
    /// families, each valid.
    ///
    /// # Errors
    ///
    /// `draft.pattern.empty` (argument `name`); `draft.pattern.limit_exceeded`;
    /// `draft.pattern.line_invalid`.
    pub fn validate(&self) -> Result<()> {
        if self.lines.is_empty() {
            return Err(Error::new(codes::PATTERN_EMPTY).with_arg("name", self.name.as_str()));
        }
        if self.lines.len() > MAX_LINES {
            return Err(limit("lines"));
        }
        self.lines.iter().try_for_each(PatternLine::validate)
    }
}

impl PatternFile {
    /// The pattern called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&HatchPattern> {
        self.patterns
            .binary_search_by(|p| p.name.as_str().cmp(name))
            .ok()
            .and_then(|i| self.patterns.get(i))
    }

    /// Reads a file from `bytes` under `limits` (`input_bytes` and `array_length` tighten the
    /// format's own bounds).
    ///
    /// # Errors
    ///
    /// `draft.pattern.syntax` (argument `line`, 1-based; 0 for bytes that are not UTF-8) for a
    /// malformed line, value or block; `draft.pattern.version_unsupported` (argument
    /// `version`); `draft.pattern.duplicate` (argument `name`); `draft.pattern.line_invalid`
    /// (arguments `line` and `reason`); `draft.pattern.empty` (argument `name`);
    /// `draft.pattern.limit_exceeded` (argument `limit`).
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let max_bytes = MAX_FILE_BYTES.min(limits.input_bytes);
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(limit("file_bytes"));
        }
        let cap = |n: usize| n.min(usize::try_from(limits.array_length).unwrap_or(usize::MAX));
        let text = std::str::from_utf8(bytes).map_err(|_| syntax(0))?;
        let mut reader = Reader {
            license: None,
            patterns: Vec::new(),
            names: BTreeSet::new(),
            open: None,
            max_patterns: cap(MAX_PATTERNS),
            max_lines: cap(MAX_LINES),
            max_dashes: cap(MAX_DASHES),
            magic: false,
        };
        for (index, raw) in text.split('\n').enumerate() {
            let line = index + 1;
            if raw.len() > MAX_LINE_BYTES {
                return Err(limit("line_bytes"));
            }
            reader.line(raw.strip_suffix('\r').unwrap_or(raw), line)?;
        }
        reader.finish(text.split('\n').count())
    }

    /// The canonical text: the magic, the license, then the patterns in name order, each with its
    /// description and families in order; numbers with at most six fraction digits, no trailing
    /// zeros.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC} {VERSION}\n");
        if let Some(license) = &self.license {
            let _ = writeln!(out, "license {license}");
        }
        for pattern in &self.patterns {
            let _ = writeln!(out, "pattern {}", pattern.name);
            if let Some(description) = &pattern.description {
                let _ = writeln!(out, "description {description}");
            }
            for line in &pattern.lines {
                out.push_str("line");
                for value in [
                    line.angle,
                    line.origin.x,
                    line.origin.y,
                    line.shift,
                    line.spacing,
                ]
                .into_iter()
                .chain(line.dashes.iter().copied())
                {
                    out.push(' ');
                    write_number(&mut out, value);
                }
                out.push('\n');
            }
            out.push_str("end\n");
        }
        out
    }
}

struct Open {
    pattern: HatchPattern,
    described: bool,
    start: usize,
}

struct Reader {
    license: Option<String>,
    patterns: Vec<HatchPattern>,
    names: BTreeSet<String>,
    open: Option<Open>,
    max_patterns: usize,
    max_lines: usize,
    max_dashes: usize,
    magic: bool,
}

impl Reader {
    fn line(&mut self, raw: &str, line: usize) -> Result<()> {
        let content = raw.trim_matches(|c: char| c == ' ' || c == '\t');
        if !self.magic {
            let mut tokens = content.split_ascii_whitespace();
            if tokens.next() != Some(MAGIC) {
                return Err(syntax(line));
            }
            let version = tokens.next().ok_or_else(|| syntax(line))?;
            if tokens.next().is_some() || version.len() > 9 {
                return Err(syntax(line));
            }
            let version: u32 = version.parse().map_err(|_| syntax(line))?;
            if version != VERSION {
                return Err(Error::new(codes::PATTERN_VERSION_UNSUPPORTED)
                    .with_arg("version", i64::from(version)));
            }
            self.magic = true;
            return Ok(());
        }
        if content.is_empty() || content.starts_with('#') {
            return Ok(());
        }
        let (keyword, rest) = content.split_once([' ', '\t']).unwrap_or((content, ""));
        let rest = rest.trim_start_matches([' ', '\t']);
        if self.open.is_some() {
            return self.block_line(keyword, rest, line);
        }
        match keyword {
            "license" => {
                if self.license.is_some() || !self.patterns.is_empty() || !text_ok(rest) {
                    return Err(syntax(line));
                }
                self.license = Some(rest.to_owned());
                Ok(())
            }
            "pattern" => self.begin(rest, line),
            _ => Err(syntax(line)),
        }
    }

    fn begin(&mut self, rest: &str, line: usize) -> Result<()> {
        if rest.contains([' ', '\t']) || !name_ok(rest) {
            return Err(syntax(line));
        }
        if self.patterns.len() >= self.max_patterns {
            return Err(limit("patterns"));
        }
        if !self.names.insert(rest.to_owned()) {
            return Err(Error::new(codes::PATTERN_DUPLICATE).with_arg("name", rest));
        }
        self.open = Some(Open {
            pattern: HatchPattern {
                name: rest.to_owned(),
                description: None,
                lines: Vec::new(),
            },
            described: false,
            start: line,
        });
        Ok(())
    }

    fn block_line(&mut self, keyword: &str, rest: &str, line: usize) -> Result<()> {
        let (max_lines, max_dashes) = (self.max_lines, self.max_dashes);
        let Some(open) = self.open.as_mut() else {
            return Err(syntax(line));
        };
        match keyword {
            "description" => {
                if open.described || !text_ok(rest) {
                    return Err(syntax(line));
                }
                open.described = true;
                open.pattern.description = Some(rest.to_owned());
            }
            "line" => {
                if open.pattern.lines.len() >= max_lines {
                    return Err(limit("lines"));
                }
                let values: Vec<f64> = rest
                    .split_ascii_whitespace()
                    .map(|t| number(t, line))
                    .collect::<Result<_>>()?;
                let [angle, x, y, shift, spacing, dashes @ ..] = values.as_slice() else {
                    return Err(syntax(line));
                };
                if dashes.len() > max_dashes {
                    return Err(limit("dashes"));
                }
                let family = PatternLine {
                    angle: *angle,
                    origin: Point2::new(*x, *y),
                    shift: *shift,
                    spacing: *spacing,
                    dashes: dashes.to_vec(),
                };
                family
                    .validate()
                    .map_err(|e| e.with_arg("line", i64::try_from(line).unwrap_or(i64::MAX)))?;
                open.pattern.lines.push(family);
            }
            "end" => {
                if !rest.is_empty() {
                    return Err(syntax(line));
                }
                if let Some(done) = self.open.take() {
                    if done.pattern.lines.is_empty() {
                        return Err(Error::new(codes::PATTERN_EMPTY)
                            .with_arg("name", done.pattern.name.as_str()));
                    }
                    self.patterns.push(done.pattern);
                }
            }
            _ => return Err(syntax(line)),
        }
        Ok(())
    }

    fn finish(mut self, lines: usize) -> Result<PatternFile> {
        if !self.magic {
            return Err(syntax(1));
        }
        if let Some(open) = &self.open {
            // A block still open at the end of the file.
            return Err(syntax(open.start.min(lines)));
        }
        self.patterns.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(PatternFile {
            license: self.license,
            patterns: self.patterns,
        })
    }
}

/// The shipped pattern library, parsed once.
///
/// # Errors
///
/// `draft.pattern.syntax` if the embedded file does not parse, which its tests rule out.
pub fn library() -> Result<Arc<PatternFile>> {
    static FILE: OnceLock<std::result::Result<Arc<PatternFile>, ()>> = OnceLock::new();
    match FILE.get_or_init(|| {
        PatternFile::parse(BUILTIN_TEXT.as_bytes(), &Limits::DEFAULT)
            .map(Arc::new)
            .map_err(|_| ())
    }) {
        Ok(file) => Ok(Arc::clone(file)),
        Err(()) => Err(syntax(0)),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const SMALL: &str = "arc-hatch-patterns 1\npattern a\nline 45 0 0 0 3\nend\n";

    fn code(text: &str) -> &'static str {
        PatternFile::parse(text.as_bytes(), &Limits::DEFAULT)
            .unwrap_err()
            .code()
    }

    #[test]
    fn numbers_follow_the_grammar() {
        assert_eq!(number("-12.5", 1).unwrap(), -12.5);
        assert!(number("-0", 1).unwrap().is_sign_positive());
        for bad in [
            "",
            "-",
            "1.",
            ".5",
            "1e3",
            "+1",
            "1234567",
            "1.2345678",
            "NaN",
            "0x1",
        ] {
            assert!(number(bad, 1).is_err(), "{bad}");
        }
    }

    #[test]
    fn canonical_text_is_fixed_and_reads_back() {
        let text = "arc-hatch-patterns 1\n# c\nlicense L\npattern b\ndescription d\nline 0.5 -0 1 2.25 -3 1 -1 0\nend\npattern a\nline 90 0 0 0 4\nend\n";
        let file = PatternFile::parse(text.as_bytes(), &Limits::DEFAULT).unwrap();
        let canonical = "arc-hatch-patterns 1\nlicense L\npattern a\nline 90 0 0 0 4\nend\npattern b\ndescription d\nline 0.5 0 1 2.25 -3 1 -1 0\nend\n";
        assert_eq!(file.to_text(), canonical);
        assert_eq!(
            PatternFile::parse(canonical.as_bytes(), &Limits::DEFAULT).unwrap(),
            file
        );
        assert_eq!(file.get("b").unwrap().lines[0].dashes, [1.0, -1.0, 0.0]);
        assert!(file.get("c").is_none());
    }

    #[test]
    fn every_error_code_has_a_golden_vector() {
        assert!(PatternFile::parse(SMALL.as_bytes(), &Limits::DEFAULT).is_ok());
        // draft.pattern.syntax
        for bad in [
            "",
            "arc-hatch-patterns\n",
            "arc-hatch-patterns 1 x\n",
            "other 1\n",
            "arc-hatch-patterns 1\nbogus\n",
            "arc-hatch-patterns 1\npattern A\n",
            "arc-hatch-patterns 1\npattern a b\n",
            "arc-hatch-patterns 1\nlicense x\nlicense y\n",
            "arc-hatch-patterns 1\npattern a\nline 45 0 0 0 3\n",
            "arc-hatch-patterns 1\npattern a\nline 45 0 0 0\nend\n",
            "arc-hatch-patterns 1\npattern a\nline 45 0 0 0 1e3\nend\n",
            "arc-hatch-patterns 1\npattern a\ndescription x\ndescription y\nline 0 0 0 0 1\nend\n",
            "arc-hatch-patterns 1\npattern a\nline 0 0 0 0 1\nend x\n",
            "arc-hatch-patterns 1\npattern a\nline 0 0 0 0 1\nend\nlicense late\n",
        ] {
            assert_eq!(code(bad), "draft.pattern.syntax", "{bad:?}");
        }
        assert_eq!(
            PatternFile::parse(&[0xff, 0xfe], &Limits::DEFAULT)
                .unwrap_err()
                .code(),
            "draft.pattern.syntax"
        );
        assert_eq!(
            code("arc-hatch-patterns 2\n"),
            "draft.pattern.version_unsupported"
        );
        assert_eq!(
            code(&format!("{SMALL}pattern a\nline 0 0 0 0 1\nend\n")),
            "draft.pattern.duplicate"
        );
        for bad in [
            "line 361 0 0 0 3",
            "line 45 0 0 0 0",
            "line 0 0 0 0 1 0 0",
            "line 0 0 0 0 1 0 -0",
        ] {
            assert_eq!(
                code(&format!("arc-hatch-patterns 1\npattern a\n{bad}\nend\n")),
                "draft.pattern.line_invalid",
                "{bad}"
            );
        }
        assert_eq!(
            code("arc-hatch-patterns 1\npattern a\nend\n"),
            "draft.pattern.empty"
        );
        let many = format!("{SMALL}{}", "line 0 0 0 0 1\n".repeat(0));
        assert!(PatternFile::parse(many.as_bytes(), &Limits::DEFAULT).is_ok());
        let lines = "line 0 0 0 0 1\n".repeat(MAX_LINES + 1);
        assert_eq!(
            code(&format!("arc-hatch-patterns 1\npattern a\n{lines}end\n")),
            "draft.pattern.limit_exceeded"
        );
        let dashes = " 1".repeat(MAX_DASHES + 1);
        assert_eq!(
            code(&format!(
                "arc-hatch-patterns 1\npattern a\nline 0 0 0 0 1{dashes}\nend\n"
            )),
            "draft.pattern.limit_exceeded"
        );
        let long = format!("arc-hatch-patterns 1\n# {}\n", "x".repeat(MAX_LINE_BYTES));
        assert_eq!(code(&long), "draft.pattern.limit_exceeded");
        let tight = Limits {
            input_bytes: 8,
            ..Limits::STRICT
        };
        assert_eq!(
            PatternFile::parse(SMALL.as_bytes(), &tight)
                .unwrap_err()
                .code(),
            "draft.pattern.limit_exceeded"
        );
        let few = Limits {
            array_length: 1,
            ..Limits::DEFAULT
        };
        let two = format!("{SMALL}pattern b\nline 0 0 0 0 1\nend\n");
        assert_eq!(
            PatternFile::parse(two.as_bytes(), &few).unwrap_err().code(),
            "draft.pattern.limit_exceeded"
        );
    }

    #[test]
    fn the_shipped_library_parses_and_is_canonical() {
        let file = library().unwrap();
        assert!(file.patterns.len() >= 10);
        for name in [
            "diagonal-lines",
            "cross-lines",
            "brick-bond",
            "concrete-mix",
            "insulation-zigzag",
            "gravel-scatter",
            "earth-fill",
            "steel-double",
            "wood-grain",
            "square-grid",
        ] {
            assert!(file.get(name).is_some(), "{name}");
        }
        assert!(file.patterns.iter().all(|p| p.validate().is_ok()));
        let again = PatternFile::parse(file.to_text().as_bytes(), &Limits::DEFAULT).unwrap();
        assert_eq!(again, *file);
    }

    proptest! {
        #[test]
        fn hostile_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            for limits in [Limits::STRICT, Limits::DEFAULT] {
                match PatternFile::parse(&bytes, &limits) {
                    Ok(file) => {
                        let back = PatternFile::parse(file.to_text().as_bytes(), &Limits::DEFAULT);
                        prop_assert_eq!(back.ok(), Some(file));
                    }
                    Err(e) => prop_assert!(e.code().starts_with("draft.pattern.")),
                }
            }
        }

        #[test]
        fn mutated_library_never_panics(at in 0usize..2000, byte in any::<u8>()) {
            let mut bytes = BUILTIN_TEXT.as_bytes().to_vec();
            if let Some(slot) = bytes.get_mut(at) {
                *slot = byte;
            }
            if let Err(e) = PatternFile::parse(&bytes, &Limits::DEFAULT) {
                prop_assert!(e.code().starts_with("draft.pattern."));
            }
        }
    }
}
