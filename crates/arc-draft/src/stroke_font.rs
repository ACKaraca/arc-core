//! The Arc stroke-font format, version 1 (CORE-DRAFT-035), and the shipped font.
//!
//! A stroke font is a line-oriented UTF-8 text file. Each glyph is a list of pen-down polylines
//! and circular arcs in font units, with an advance width; the font carries its metrics and
//! optional kerning pairs. The format is specified in
//! [`docs/architecture/stroke-font-format.md`](../../../docs/architecture/stroke-font-format.md);
//! [`StrokeFont::parse`] is its bounded reader and [`StrokeFont::to_text`] writes the canonical
//! form, which the reader reads back to an equal font.
//!
//! [`builtin`] is the shipped font, *Arc Stroke*, an original work of the project.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result};

use crate::error::codes;

/// The first token of a stroke-font file.
pub const MAGIC: &str = "arc-stroke-font";
/// The format version this reader reads and the writer writes.
pub const VERSION: u32 = 1;
/// The most bytes of a file, whatever the limits allow.
pub const MAX_FILE_BYTES: u64 = 16 << 20;
/// The most bytes of one line.
pub const MAX_LINE_BYTES: usize = 4096;
/// The most glyphs of a font.
pub const MAX_GLYPHS: usize = 65_536;
/// The most elements (polylines and arcs) of one glyph.
pub const MAX_ELEMENTS: usize = 256;
/// The most points of one polyline.
pub const MAX_POINTS: usize = 256;
/// The most kerning pairs of a font.
pub const MAX_KERNING: usize = 65_536;
/// The largest units-per-em value.
pub const MAX_UNITS: f64 = 16_384.0;
/// Coordinates, metrics and advances lie within this many em of zero.
const RANGE_EM: f64 = 4.0;
/// The bounds check allows this fraction of a unit beyond the glyph box.
const BOUNDS_SLACK: f64 = 1e-9;

/// The name of the shipped font.
pub const BUILTIN_NAME: &str = "Arc Stroke";
const BUILTIN_TEXT: &str = include_str!("../fonts/arc-stroke.asf");

/// One pen-down element of a glyph, in font units with y up and the origin on the baseline at
/// the left edge of the advance.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// A polyline through at least two points.
    Polyline(Vec<Point2>),
    /// A circular arc: `start` and `sweep` in degrees, counterclockwise positive.
    Arc {
        /// The center.
        center: Point2,
        /// The radius, positive.
        radius: f64,
        /// The angle of the start point from the positive x axis.
        start: f64,
        /// The signed sweep, not zero and at most 360 in magnitude.
        sweep: f64,
    },
}

/// One glyph: its advance width and pen-down elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    /// The advance width in font units.
    pub advance: f64,
    /// The elements in drawing order.
    pub elements: Vec<Element>,
}

/// A parsed stroke font.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeFont {
    /// The font name.
    pub name: String,
    /// The license statement, if any.
    pub license: Option<String>,
    /// The units per em: the scale reference of every coordinate.
    pub units: f64,
    /// The height of capital letters above the baseline; text height maps to it.
    pub cap_height: f64,
    /// The height of lowercase letters without ascenders.
    pub x_height: f64,
    /// The top of the glyph box, positive.
    pub ascender: f64,
    /// The bottom of the glyph box, zero or negative.
    pub descender: f64,
    /// The default distance between baselines.
    pub line_spacing: f64,
    /// The glyph drawn for characters the font lacks, if any.
    pub missing: Option<char>,
    /// The glyphs by Unicode scalar value.
    pub glyphs: BTreeMap<char, Glyph>,
    /// Advance adjustments between consecutive characters, in font units.
    pub kerning: BTreeMap<(char, char), f64>,
}

fn syntax(line: usize) -> Error {
    Error::new(codes::FONT_SYNTAX).with_arg("line", i64::try_from(line).unwrap_or(i64::MAX))
}

fn limit(name: &str) -> Error {
    Error::new(codes::FONT_LIMIT_EXCEEDED).with_arg("limit", name)
}

fn scalar_name(c: char) -> String {
    format!("U+{:04X}", u32::from(c))
}

/// A decimal number: an optional minus sign, 1 to 6 digits, optionally a point and 1 to 6
/// digits.
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
    token.parse::<f64>().map_err(|_| syntax(line))
}

fn scalar(token: &str, line: usize) -> Result<char> {
    if token.is_empty() || token.len() > 6 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(syntax(line));
    }
    u32::from_str_radix(token, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| syntax(line))
}

/// The extreme points of an element: polyline points, or the arc's ends and the points where
/// it crosses the axis directions within its sweep.
fn extremes(element: &Element) -> Vec<Point2> {
    match element {
        Element::Polyline(points) => points.clone(),
        Element::Arc {
            center,
            radius,
            start,
            sweep,
        } => {
            let at = |deg: f64| {
                let (s, c) = deg.to_radians().sin_cos();
                Point2::new(center.x + radius * c, center.y + radius * s)
            };
            let (lo, hi) = if *sweep >= 0.0 {
                (*start, start + sweep)
            } else {
                (start + sweep, *start)
            };
            let mut points = vec![at(*start), at(start + sweep)];
            let mut k = (lo / 90.0).ceil();
            // At most 6 quarter turns lie in a sweep of 360 degrees.
            for _ in 0..6 {
                if k * 90.0 > hi {
                    break;
                }
                let axis = [
                    Point2::new(center.x + radius, center.y),
                    Point2::new(center.x, center.y + radius),
                    Point2::new(center.x - radius, center.y),
                    Point2::new(center.x, center.y - radius),
                ];
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "rem_euclid(4.0) is in 0..4"
                )]
                let quarter = k.rem_euclid(4.0) as usize;
                points.extend(axis.get(quarter).copied());
                k += 1.0;
            }
            points
        }
    }
}

impl StrokeFont {
    /// The glyph of `c`, if the font has one.
    #[must_use]
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c)
    }

    /// The kerning adjustment between `left` and `right`, 0 when none is declared.
    #[must_use]
    pub fn kern(&self, left: char, right: char) -> f64 {
        self.kerning.get(&(left, right)).copied().unwrap_or(0.0)
    }

    /// Whether every glyph lies inside its box: x from 0 to its advance, y from the descender
    /// to the ascender, with a slack of 10⁻⁹ unit for the arc extremes.
    ///
    /// # Errors
    ///
    /// `draft.font.glyph_out_of_bounds` with the argument `glyph` (`U+XXXX`) for the first
    /// glyph that leaves its box.
    pub fn check_bounds(&self) -> Result<()> {
        let slack = BOUNDS_SLACK * self.units;
        for (c, glyph) in &self.glyphs {
            let inside = glyph.elements.iter().flat_map(extremes).all(|p| {
                p.x >= -slack
                    && p.x <= glyph.advance + slack
                    && p.y >= self.descender - slack
                    && p.y <= self.ascender + slack
            });
            if !inside {
                return Err(
                    Error::new(codes::FONT_GLYPH_OUT_OF_BOUNDS).with_arg("glyph", scalar_name(*c))
                );
            }
        }
        Ok(())
    }

    /// Reads a font from `bytes` under `limits` (`input_bytes` and `array_length` tighten the
    /// format's own bounds).
    ///
    /// # Errors
    ///
    /// `draft.font.syntax` (argument `line`, 1-based; 0 for bytes that are not UTF-8) for a
    /// malformed line or value; `draft.font.version_unsupported` (argument `version`);
    /// `draft.font.header_invalid` (argument `key`) for a missing, repeated or inconsistent
    /// header value; `draft.font.glyph_duplicate`, `draft.font.glyph_unknown` (argument
    /// `glyph`) for a repeated glyph or a reference to an absent one;
    /// `draft.font.glyph_out_of_bounds`; `draft.font.limit_exceeded` (argument `limit`).
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let max_bytes = MAX_FILE_BYTES.min(limits.input_bytes);
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(limit("file_bytes"));
        }
        let cap = |n: usize| n.min(usize::try_from(limits.array_length).unwrap_or(usize::MAX));
        let text = std::str::from_utf8(bytes).map_err(|_| syntax(0))?;
        let mut reader = Reader {
            header: BTreeMap::new(),
            glyphs: BTreeMap::new(),
            kerning: BTreeMap::new(),
            open: None,
            max_glyphs: cap(MAX_GLYPHS),
            max_kerning: cap(MAX_KERNING),
            max_points: cap(MAX_POINTS),
            max_elements: cap(MAX_ELEMENTS),
            magic: false,
        };
        for (index, raw) in text.split('\n').enumerate() {
            let line = index + 1;
            if raw.len() > MAX_LINE_BYTES {
                return Err(limit("line_bytes"));
            }
            reader.line(raw.strip_suffix('\r').unwrap_or(raw), line)?;
        }
        reader.finish()
    }

    /// The canonical text of the font: the header in a fixed order, glyphs in scalar order,
    /// kerning pairs in pair order, numbers in their shortest decimal form.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC} {VERSION}\nname {}\n", self.name);
        if let Some(license) = &self.license {
            let _ = writeln!(out, "license {license}");
        }
        for (key, value) in [
            ("units", self.units),
            ("cap-height", self.cap_height),
            ("x-height", self.x_height),
            ("ascender", self.ascender),
            ("descender", self.descender),
            ("line-spacing", self.line_spacing),
        ] {
            let _ = writeln!(out, "{key} {value}");
        }
        if let Some(missing) = self.missing {
            let _ = writeln!(out, "missing {:x}", u32::from(missing));
        }
        for (c, glyph) in &self.glyphs {
            let _ = writeln!(out, "glyph {:x} {}", u32::from(*c), glyph.advance);
            for element in &glyph.elements {
                match element {
                    Element::Polyline(points) => {
                        out.push('l');
                        for p in points {
                            let _ = write!(out, " {} {}", p.x, p.y);
                        }
                        out.push('\n');
                    }
                    Element::Arc {
                        center,
                        radius,
                        start,
                        sweep,
                    } => {
                        let _ =
                            writeln!(out, "a {} {} {radius} {start} {sweep}", center.x, center.y);
                    }
                }
            }
            out.push_str("end\n");
        }
        for ((a, b), value) in &self.kerning {
            let _ = writeln!(out, "kern {:x} {:x} {value}", u32::from(*a), u32::from(*b));
        }
        out
    }
}

const HEADER_KEYS: [&str; 9] = [
    "name",
    "license",
    "units",
    "cap-height",
    "x-height",
    "ascender",
    "descender",
    "line-spacing",
    "missing",
];

struct Reader {
    header: BTreeMap<&'static str, String>,
    glyphs: BTreeMap<char, Glyph>,
    kerning: BTreeMap<(char, char), (f64, usize)>,
    open: Option<(char, Glyph, usize)>,
    max_glyphs: usize,
    max_kerning: usize,
    max_points: usize,
    max_elements: usize,
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
                return Err(Error::new(codes::FONT_VERSION_UNSUPPORTED)
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
            return self.glyph_line(keyword, rest, line);
        }
        match keyword {
            "glyph" => self.begin_glyph(rest, line),
            "kern" => self.kern(rest, line),
            key => {
                let key = HEADER_KEYS
                    .iter()
                    .find(|k| **k == key)
                    .ok_or_else(|| syntax(line))?;
                if !self.glyphs.is_empty() || !self.kerning.is_empty() {
                    return Err(Error::new(codes::FONT_HEADER_INVALID).with_arg("key", *key));
                }
                if rest.is_empty()
                    || self.header.insert(key, rest.to_owned()).is_some()
                    || rest.chars().any(char::is_control)
                {
                    return Err(Error::new(codes::FONT_HEADER_INVALID).with_arg("key", *key));
                }
                Ok(())
            }
        }
    }

    fn numbers(rest: &str, line: usize) -> Result<Vec<f64>> {
        rest.split_ascii_whitespace()
            .map(|t| number(t, line))
            .collect()
    }

    fn begin_glyph(&mut self, rest: &str, line: usize) -> Result<()> {
        let mut tokens = rest.split_ascii_whitespace();
        let (Some(code), Some(advance), None) = (tokens.next(), tokens.next(), tokens.next())
        else {
            return Err(syntax(line));
        };
        let c = scalar(code, line)?;
        let advance = number(advance, line)?;
        if self.glyphs.len() >= self.max_glyphs {
            return Err(limit("glyphs"));
        }
        if self.glyphs.contains_key(&c) {
            return Err(Error::new(codes::FONT_GLYPH_DUPLICATE).with_arg("glyph", scalar_name(c)));
        }
        self.open = Some((
            c,
            Glyph {
                advance,
                elements: Vec::new(),
            },
            line,
        ));
        Ok(())
    }

    fn glyph_line(&mut self, keyword: &str, rest: &str, line: usize) -> Result<()> {
        let max_points = self.max_points;
        let max_elements = self.max_elements;
        let Some((_, glyph, _)) = self.open.as_mut() else {
            return Err(syntax(line));
        };
        let element = match keyword {
            "end" => {
                if !rest.is_empty() {
                    return Err(syntax(line));
                }
                if let Some((c, glyph, _)) = self.open.take() {
                    self.glyphs.insert(c, glyph);
                }
                return Ok(());
            }
            "l" => {
                if rest.split_ascii_whitespace().count() > 2 * max_points {
                    return Err(limit("points"));
                }
                let values = Self::numbers(rest, line)?;
                if values.len() < 4 || values.len() % 2 != 0 {
                    return Err(syntax(line));
                }
                Element::Polyline(
                    values
                        .chunks_exact(2)
                        .map(|p| match p {
                            [x, y] => Point2::new(*x, *y),
                            _ => Point2::origin(),
                        })
                        .collect(),
                )
            }
            "a" => match Self::numbers(rest, line)?.as_slice() {
                [x, y, radius, start, sweep]
                    if *radius > 0.0
                        && *sweep != 0.0
                        && sweep.abs() <= 360.0
                        && start.abs() <= 360.0 =>
                {
                    Element::Arc {
                        center: Point2::new(*x, *y),
                        radius: *radius,
                        start: *start,
                        sweep: *sweep,
                    }
                }
                _ => return Err(syntax(line)),
            },
            _ => return Err(syntax(line)),
        };
        if glyph.elements.len() >= max_elements {
            return Err(limit("elements"));
        }
        glyph.elements.push(element);
        Ok(())
    }

    fn kern(&mut self, rest: &str, line: usize) -> Result<()> {
        let mut tokens = rest.split_ascii_whitespace();
        let (Some(a), Some(b), Some(value), None) =
            (tokens.next(), tokens.next(), tokens.next(), tokens.next())
        else {
            return Err(syntax(line));
        };
        let pair = (scalar(a, line)?, scalar(b, line)?);
        let value = number(value, line)?;
        if self.kerning.len() >= self.max_kerning {
            return Err(limit("kerning"));
        }
        if self.kerning.insert(pair, (value, line)).is_some() {
            return Err(syntax(line));
        }
        Ok(())
    }

    fn metric(&self, key: &'static str) -> Result<f64> {
        let invalid = || Error::new(codes::FONT_HEADER_INVALID).with_arg("key", key);
        let value = self.header.get(key).ok_or_else(invalid)?;
        number(value, 0).map_err(|_| invalid())
    }

    fn finish(self) -> Result<StrokeFont> {
        if let Some((_, _, line)) = self.open {
            return Err(syntax(line));
        }
        if !self.magic {
            return Err(syntax(1));
        }
        let invalid = |key: &str| Error::new(codes::FONT_HEADER_INVALID).with_arg("key", key);
        let name = self.header.get("name").ok_or_else(|| invalid("name"))?;
        let units = self.metric("units")?;
        if !(units > 0.0 && units <= MAX_UNITS) {
            return Err(invalid("units"));
        }
        let range = RANGE_EM * units;
        let mut metrics = Vec::new();
        for key in [
            "cap-height",
            "x-height",
            "ascender",
            "descender",
            "line-spacing",
        ] {
            let value = self.metric(key)?;
            if value.abs() > range {
                return Err(invalid(key));
            }
            metrics.push(value);
        }
        let [cap_height, x_height, ascender, descender, line_spacing] = metrics[..] else {
            return Err(invalid("units"));
        };
        if cap_height <= 0.0 {
            return Err(invalid("cap-height"));
        }
        if x_height <= 0.0 {
            return Err(invalid("x-height"));
        }
        if ascender < cap_height {
            return Err(invalid("ascender"));
        }
        if descender > 0.0 {
            return Err(invalid("descender"));
        }
        if line_spacing <= 0.0 {
            return Err(invalid("line-spacing"));
        }
        for (c, glyph) in &self.glyphs {
            let far = |p: &Point2| p.x.abs() > range || p.y.abs() > range;
            let bad = glyph.advance < 0.0
                || glyph.advance > range
                || glyph.elements.iter().any(|e| match e {
                    Element::Polyline(points) => points.iter().any(far),
                    Element::Arc { center, radius, .. } => far(center) || *radius > range,
                });
            if bad {
                return Err(
                    Error::new(codes::FONT_GLYPH_OUT_OF_BOUNDS).with_arg("glyph", scalar_name(*c))
                );
            }
        }
        let known = |c: char| -> Result<char> {
            if self.glyphs.contains_key(&c) {
                Ok(c)
            } else {
                Err(Error::new(codes::FONT_GLYPH_UNKNOWN).with_arg("glyph", scalar_name(c)))
            }
        };
        let missing = match self.header.get("missing") {
            Some(code) => Some(known(scalar(code, 0).map_err(|_| invalid("missing"))?)?),
            None => None,
        };
        let mut kerning = BTreeMap::new();
        for ((a, b), (value, line)) in &self.kerning {
            known(*a)?;
            known(*b)?;
            if value.abs() > range {
                return Err(syntax(*line));
            }
            kerning.insert((*a, *b), *value);
        }
        let font = StrokeFont {
            name: name.clone(),
            license: self.header.get("license").cloned(),
            units,
            cap_height,
            x_height,
            ascender,
            descender,
            line_spacing,
            missing,
            glyphs: self.glyphs,
            kerning,
        };
        font.check_bounds()?;
        Ok(font)
    }
}

/// The shipped font *Arc Stroke*, parsed once.
///
/// # Errors
///
/// The reader's error if the embedded file does not parse, which its tests rule out.
pub fn builtin() -> Result<Arc<StrokeFont>> {
    static FONT: OnceLock<std::result::Result<Arc<StrokeFont>, String>> = OnceLock::new();
    match FONT.get_or_init(|| {
        StrokeFont::parse(BUILTIN_TEXT.as_bytes(), &Limits::DEFAULT)
            .map(Arc::new)
            .map_err(|e| e.code().to_owned())
    }) {
        Ok(font) => Ok(Arc::clone(font)),
        Err(_) => Err(Error::new(codes::FONT_DEFAULT_MISSING).with_arg("name", BUILTIN_NAME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "arc-stroke-font 1\nname T\nunits 10\ncap-height 5\nx-height 3\n\
        ascender 8\ndescender -2\nline-spacing 9\n";

    fn parse(text: &str) -> Result<StrokeFont> {
        StrokeFont::parse(text.as_bytes(), &Limits::DEFAULT)
    }

    #[test]
    fn numbers_follow_the_grammar() {
        assert_eq!(number("-12.5", 1).unwrap(), -12.5);
        for bad in [
            "",
            "-",
            "1.",
            ".5",
            "1e3",
            "+1",
            "1234567",
            "0x1",
            "1.2345678",
            "NaN",
        ] {
            assert!(number(bad, 1).is_err(), "{bad}");
        }
    }

    #[test]
    fn arc_extremes_include_axis_crossings() {
        let arc = Element::Arc {
            center: Point2::new(0.0, 0.0),
            radius: 1.0,
            start: -45.0,
            sweep: 90.0,
        };
        let xs: Vec<f64> = extremes(&arc).iter().map(|p| p.x).collect();
        assert!(xs.iter().any(|x| (x - 1.0).abs() < 1e-15));
        let full = Element::Arc {
            center: Point2::new(0.0, 0.0),
            radius: 1.0,
            start: 0.0,
            sweep: -360.0,
        };
        assert!(extremes(&full).iter().any(|p| (p.y + 1.0).abs() < 1e-15));
    }

    #[test]
    fn header_rules_are_enforced() {
        assert!(parse(SMALL).is_ok());
        let code = |t: &str| parse(t).unwrap_err().code();
        assert_eq!(
            code(&SMALL.replace("units 10", "units 0")),
            "draft.font.header_invalid"
        );
        assert_eq!(
            code(&format!("{SMALL}name again\n")),
            "draft.font.header_invalid"
        );
        assert_eq!(
            code(&SMALL.replace(" 1\n", " 2\n")),
            "draft.font.version_unsupported"
        );
        assert_eq!(code(&format!("{SMALL}bogus 1\n")), "draft.font.syntax");
        assert_eq!(
            code(&format!("{SMALL}missing 41\n")),
            "draft.font.glyph_unknown"
        );
    }
}
