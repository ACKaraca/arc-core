//! Single-line text and multi-line rich text (CORE-DRAFT-006).
//!
//! Rich text is structured data: paragraphs with alignment, indents, spacing, line spacing and
//! tab stops, each holding runs with a font name, height, color, weight and slant flags,
//! underline, overline, strike-through and an optional stacked fraction. There is no inline
//! formatting-code language: the structure is the stored form. [`paragraphs_from_plain`] turns
//! plain text with line breaks into paragraphs.
//!
//! Counts and sizes are bounded by [`arc_base::Limits`]: the paragraphs of a text, the runs of
//! a paragraph and the tab stops of a paragraph by `array_length`, the text bytes of an entity
//! by `string_bytes` ([`validate`]).

use arc_base::{Color, Error, Limits, Result};
use arc_doc::{Bound, ColorValue, FieldDecl, Finding, TypeValidator, Value, ValueType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::fields::{self, Field};
use crate::style::{TEXT_STYLE, style_reference};
use crate::types::{TEXT_BYTES, f64_at, finding, flag, real, variants};

/// A single-line text: common fields, then `content`, `position`, `rotation`, `style`,
/// `height`, `halign`, `valign`.
pub const TEXT: &str = "core.draft.text";
/// A multi-line rich text: common fields, then `paragraphs`, `position`, `rotation`, `style`,
/// `height`, `width`, `attachment`, `columns`, `column_gutter`, `column_height`.
pub const RICH_TEXT: &str = "core.draft.rich_text";

/// The text height used when neither the text nor its style sets one.
pub const DEFAULT_HEIGHT: f64 = 2.5;
/// The most columns of a rich text.
pub const MAX_COLUMNS: i64 = 64;

/// Field indices of text entities (after the common fields).
pub mod field {
    use crate::types::entity::KIND;
    /// `content` of a single-line text; `paragraphs` of a rich text.
    pub const CONTENT: usize = KIND;
    /// `position`: the attachment point in the container.
    pub const POSITION: usize = KIND + 1;
    /// `rotation` in radians.
    pub const ROTATION: usize = KIND + 2;
    /// `style`: the text style.
    pub const STYLE: usize = KIND + 3;
    /// `height`: the text height; 0 takes the style's.
    pub const HEIGHT: usize = KIND + 4;
    /// `halign` of a single-line text; `width` of a rich text.
    pub const HALIGN: usize = KIND + 5;
    /// `width` of a rich text: the wrapping width, 0 for none.
    pub const WIDTH: usize = KIND + 5;
    /// `valign` of a single-line text; `attachment` of a rich text.
    pub const VALIGN: usize = KIND + 6;
    /// `fields` of a single-line text (CORE-DRAFT-007).
    pub const FIELDS: usize = KIND + 7;
    /// `attachment` of a rich text.
    pub const ATTACHMENT: usize = KIND + 6;
    /// `columns` of a rich text.
    pub const COLUMNS: usize = KIND + 7;
    /// `column_gutter` of a rich text.
    pub const COLUMN_GUTTER: usize = KIND + 8;
    /// `column_height` of a rich text.
    pub const COLUMN_HEIGHT: usize = KIND + 9;
}

/// Field indices of a text style.
pub mod style_field {
    /// `font`.
    pub const FONT: usize = 1;
    /// `height`.
    pub const HEIGHT: usize = 2;
    /// `width_factor`.
    pub const WIDTH_FACTOR: usize = 3;
    /// `oblique` in radians.
    pub const OBLIQUE: usize = 4;
    /// `backwards`.
    pub const BACKWARDS: usize = 5;
    /// `upside_down`.
    pub const UPSIDE_DOWN: usize = 6;
}

/// The alignment of a paragraph's lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    /// Lines start at the left edge.
    #[default]
    Left,
    /// Lines are centered.
    Center,
    /// Lines end at the right edge.
    Right,
    /// Lines other than the last of the paragraph end at the right edge too.
    Justify,
}

/// How a run is stacked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stack {
    /// Ordinary text.
    #[default]
    None,
    /// The text over the denominator, with a horizontal bar.
    Horizontal,
    /// The text and the denominator side by side, with a slash.
    Diagonal,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn one() -> f64 {
    1.0
}

/// One run of a paragraph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// The text; a tab advances to the next tab stop. No other control characters.
    pub text: String,
    /// The font name; absent takes the text style's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// The height; absent takes the text's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// The linear RGBA color; absent takes the entity's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
    /// Bold weight.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    /// Slanted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Underlined.
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Overlined.
    #[serde(default, skip_serializing_if = "is_false")]
    pub overline: bool,
    /// Struck through.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    /// Stacked fraction: `text` is the numerator.
    #[serde(default)]
    pub stack: Stack,
    /// The denominator of a stacked fraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<String>,
    /// Fields inserted into `text` (CORE-DRAFT-007); the layout reads the evaluated text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
}

/// One paragraph of a rich text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Paragraph {
    /// The alignment.
    #[serde(default)]
    pub alignment: Alignment,
    /// The indent of the first line from the left edge.
    #[serde(default)]
    pub indent_first: f64,
    /// The indent of the other lines.
    #[serde(default)]
    pub indent: f64,
    /// Extra space above the paragraph.
    #[serde(default)]
    pub space_before: f64,
    /// Extra space below the paragraph.
    #[serde(default)]
    pub space_after: f64,
    /// The factor on the font's line spacing.
    #[serde(default = "one")]
    pub line_spacing: f64,
    /// Tab stops from the left edge, ascending and positive; past them, stops every four text
    /// heights.
    #[serde(default)]
    pub tabs: Vec<f64>,
    /// The runs.
    #[serde(default)]
    pub runs: Vec<Run>,
}

impl Default for Paragraph {
    fn default() -> Self {
        Self {
            alignment: Alignment::Left,
            indent_first: 0.0,
            indent: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            line_spacing: 1.0,
            tabs: Vec::new(),
            runs: Vec::new(),
        }
    }
}

fn opt_string(value: &Option<String>) -> Option<Value> {
    value.clone().map(Value::String)
}

impl Run {
    fn to_value(&self) -> Value {
        Value::Record(vec![
            Some(Value::String(self.text.clone())),
            opt_string(&self.font),
            self.height.map(Value::F64),
            self.color.map(|[r, g, b, a]| {
                Value::Color(Box::new(ColorValue {
                    rgba: Color::new(r, g, b, a),
                    source: None,
                }))
            }),
            Some(Value::Bool(self.bold)),
            Some(Value::Bool(self.italic)),
            Some(Value::Bool(self.underline)),
            Some(Value::Bool(self.overline)),
            Some(Value::Bool(self.strike)),
            Some(Value::Enum(self.stack as u32)),
            opt_string(&self.denominator),
            Some(fields::list_value(&self.fields)),
        ])
    }

    fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let string = |i: usize| match f.get(i) {
            Some(Some(Value::String(s))) => Some(s.clone()),
            _ => None,
        };
        let flag = |i: usize| matches!(f.get(i), Some(Some(Value::Bool(true))));
        Some(Self {
            text: string(0).unwrap_or_default(),
            font: string(1),
            height: f64_at(f, 2),
            color: match f.get(3) {
                Some(Some(Value::Color(c))) => Some([c.rgba.r, c.rgba.g, c.rgba.b, c.rgba.a]),
                _ => None,
            },
            bold: flag(4),
            italic: flag(5),
            underline: flag(6),
            overline: flag(7),
            strike: flag(8),
            stack: match f.get(9) {
                Some(Some(Value::Enum(1))) => Stack::Horizontal,
                Some(Some(Value::Enum(2))) => Stack::Diagonal,
                _ => Stack::None,
            },
            denominator: string(10),
            fields: fields::fields_of(f.get(11).and_then(Option::as_ref)),
        })
    }
}

impl Paragraph {
    /// The stored form of the paragraph.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Record(vec![
            Some(Value::Enum(self.alignment as u32)),
            Some(Value::F64(self.indent_first)),
            Some(Value::F64(self.indent)),
            Some(Value::F64(self.space_before)),
            Some(Value::F64(self.space_after)),
            Some(Value::F64(self.line_spacing)),
            Some(Value::List(
                self.tabs.iter().map(|t| Value::F64(*t)).collect(),
            )),
            Some(Value::List(self.runs.iter().map(Run::to_value).collect())),
        ])
    }

    /// The paragraph of a stored value; `None` when it is not a paragraph record.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let list = |i: usize| match f.get(i) {
            Some(Some(Value::List(items))) => items.as_slice(),
            _ => &[],
        };
        Some(Self {
            alignment: match f.first() {
                Some(Some(Value::Enum(1))) => Alignment::Center,
                Some(Some(Value::Enum(2))) => Alignment::Right,
                Some(Some(Value::Enum(3))) => Alignment::Justify,
                _ => Alignment::Left,
            },
            indent_first: f64_at(f, 1).unwrap_or(0.0),
            indent: f64_at(f, 2).unwrap_or(0.0),
            space_before: f64_at(f, 3).unwrap_or(0.0),
            space_after: f64_at(f, 4).unwrap_or(0.0),
            line_spacing: f64_at(f, 5).unwrap_or(1.0),
            tabs: list(6)
                .iter()
                .filter_map(|v| match v {
                    Value::F64(x) => Some(*x),
                    _ => None,
                })
                .collect(),
            runs: list(7).iter().filter_map(Run::from_value).collect(),
        })
    }
}

/// The paragraphs of a rich text's field values.
#[must_use]
pub fn paragraphs_of(values: &[Option<Value>]) -> Vec<Paragraph> {
    match values.get(field::CONTENT) {
        Some(Some(Value::List(items))) => items.iter().filter_map(Paragraph::from_value).collect(),
        _ => Vec::new(),
    }
}

/// The stored `paragraphs` value.
#[must_use]
pub fn paragraphs_value(paragraphs: &[Paragraph]) -> Value {
    Value::List(paragraphs.iter().map(Paragraph::to_value).collect())
}

/// Plain text as paragraphs: one per line (`\n`, `\r\n` or `\r` ends a line), each with one
/// run of the line's text, or none for an empty line.
///
/// # Errors
///
/// As [`validate`].
pub fn paragraphs_from_plain(text: &str, limits: &Limits) -> Result<Vec<Paragraph>> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let paragraphs: Vec<Paragraph> = normalized
        .split('\n')
        .map(|line| Paragraph {
            runs: if line.is_empty() {
                Vec::new()
            } else {
                vec![Run {
                    text: line.to_owned(),
                    ..Run::default()
                }]
            },
            ..Paragraph::default()
        })
        .collect();
    validate(&paragraphs, limits)?;
    Ok(paragraphs)
}

fn too_large(limit: &str) -> Error {
    Error::new(codes::TEXT_TOO_LARGE).with_arg("limit", limit)
}

/// Whether `text` holds only characters a run may hold: no control character except tab.
#[must_use]
pub fn content_ok(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() && c != '\t')
}

fn fits(count: usize, max: u64) -> bool {
    u64::try_from(count).is_ok_and(|n| n <= max)
}

/// Checks paragraphs against the rules and `limits`.
///
/// # Errors
///
/// `draft.text.too_large` (argument `limit`: `paragraphs`, `runs`, `tabs` or `bytes`);
/// `draft.text.content_invalid` for a control character other than tab, a non-finite or
/// negative size, or a line spacing that is not positive; `draft.text.stack_invalid` for a
/// stacked run without a denominator, or a denominator on an ordinary run;
/// `draft.text.tabs_invalid` for tab stops that are not finite, positive and ascending.
pub fn validate(paragraphs: &[Paragraph], limits: &Limits) -> Result<()> {
    if !fits(paragraphs.len(), limits.array_length) {
        return Err(too_large("paragraphs"));
    }
    let mut bytes: u64 = 0;
    for p in paragraphs {
        if !fits(p.runs.len(), limits.array_length) {
            return Err(too_large("runs"));
        }
        if !fits(p.tabs.len(), limits.array_length) {
            return Err(too_large("tabs"));
        }
        let sizes = [p.indent_first, p.indent, p.space_before, p.space_after];
        if sizes.iter().any(|s| !s.is_finite() || *s < 0.0)
            || !(p.line_spacing.is_finite() && p.line_spacing > 0.0)
        {
            return Err(Error::new(codes::TEXT_CONTENT_INVALID));
        }
        let ascending = p.tabs.windows(2).all(|w| matches!(w, [a, b] if a < b));
        if !ascending || p.tabs.iter().any(|t| !t.is_finite() || *t <= 0.0) {
            return Err(Error::new(codes::TEXT_TABS_INVALID));
        }
        for run in &p.runs {
            let denominator = run.denominator.as_deref().unwrap_or("");
            bytes = bytes.saturating_add(
                u64::try_from(run.text.len() + denominator.len()).unwrap_or(u64::MAX),
            );
            if !content_ok(&run.text) || !content_ok(denominator) {
                return Err(Error::new(codes::TEXT_CONTENT_INVALID));
            }
            if run.height.is_some_and(|h| !(h.is_finite() && h > 0.0)) {
                return Err(Error::new(codes::TEXT_CONTENT_INVALID));
            }
            if (run.stack == Stack::None) == run.denominator.is_some() {
                return Err(Error::new(codes::TEXT_STACK_INVALID));
            }
            fields::validate(&run.text, &run.fields, limits)?;
        }
    }
    if bytes > limits.string_bytes {
        return Err(too_large("bytes"));
    }
    Ok(())
}

fn size(name: &str) -> FieldDecl {
    real(name, 0.0, 0.0, f64::MAX)
}

fn placement() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("position", ValueType::Point2)
            .with_default(Value::Point2(arc_base::math::Point2::origin())),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("style", style_reference(TEXT_STYLE, true)).optional(),
        size("height"),
    ]
}

/// The fields of a single-line text after the common ones.
pub(crate) fn text_fields() -> Vec<FieldDecl> {
    let mut fields = vec![
        FieldDecl::new("content", ValueType::String).with_bound(Bound::Length { max: TEXT_BYTES }),
    ];
    fields.extend(placement());
    fields.push(
        FieldDecl::new("halign", variants(&["left", "center", "right"]))
            .with_default(Value::Enum(0)),
    );
    fields.push(
        FieldDecl::new("valign", variants(&["baseline", "bottom", "middle", "top"]))
            .with_default(Value::Enum(0)),
    );
    fields.push(fields::list_decl());
    fields
}

fn run_type() -> ValueType {
    let text = |name: &str| FieldDecl::new(name, ValueType::String).optional();
    ValueType::Record(vec![
        FieldDecl::new("text", ValueType::String),
        text("font"),
        FieldDecl::new("height", ValueType::F64)
            .with_bound(Bound::Real {
                min: f64::MIN_POSITIVE,
                max: f64::MAX,
            })
            .optional(),
        FieldDecl::new("color", ValueType::Color).optional(),
        flag("bold", false),
        flag("italic", false),
        flag("underline", false),
        flag("overline", false),
        flag("strike", false),
        FieldDecl::new("stack", variants(&["none", "horizontal", "diagonal"]))
            .with_default(Value::Enum(0)),
        text("denominator"),
        fields::list_decl(),
    ])
}

fn paragraph_type() -> ValueType {
    let array = Limits::DEFAULT.array_length;
    ValueType::Record(vec![
        FieldDecl::new(
            "alignment",
            variants(&["left", "center", "right", "justify"]),
        )
        .with_default(Value::Enum(0)),
        size("indent_first"),
        size("indent"),
        size("space_before"),
        size("space_after"),
        real("line_spacing", 1.0, f64::MIN_POSITIVE, f64::MAX),
        FieldDecl::new("tabs", ValueType::List(Box::new(ValueType::F64)))
            .with_bound(Bound::Length { max: array }),
        FieldDecl::new("runs", ValueType::List(Box::new(run_type())))
            .with_bound(Bound::Length { max: array }),
    ])
}

/// The fields of a rich text after the common ones.
pub(crate) fn rich_text_fields() -> Vec<FieldDecl> {
    let mut fields = vec![
        FieldDecl::new("paragraphs", ValueType::List(Box::new(paragraph_type()))).with_bound(
            Bound::Length {
                max: Limits::DEFAULT.array_length,
            },
        ),
    ];
    fields.extend(placement());
    fields.push(size("width"));
    fields.push(
        FieldDecl::new(
            "attachment",
            variants(&[
                "top_left",
                "top_center",
                "top_right",
                "middle_left",
                "middle_center",
                "middle_right",
                "bottom_left",
                "bottom_center",
                "bottom_right",
            ]),
        )
        .with_default(Value::Enum(0)),
    );
    fields.push(
        FieldDecl::new("columns", ValueType::I64)
            .with_default(Value::I64(1))
            .with_bound(Bound::Int {
                min: 1,
                max: MAX_COLUMNS,
            }),
    );
    fields.push(size("column_gutter"));
    fields.push(size("column_height"));
    fields
}

/// The type validator of a single-line text: the common findings and the content rule.
pub(crate) fn text_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = crate::types::common_findings(values);
    if let Some(Some(Value::String(s))) = values.get(field::CONTENT)
        && !content_ok(s)
    {
        findings.push(finding(Error::new(codes::TEXT_CONTENT_INVALID)));
    }
    if let Some(Some(Value::String(s))) = values.get(field::CONTENT)
        && let Err(error) = fields::validate(
            s,
            &fields::fields_of(values.get(field::FIELDS).and_then(Option::as_ref)),
            &Limits::DEFAULT,
        )
    {
        findings.push(finding(error));
    }
    findings
}

/// The type validator of a rich text: the common findings and [`validate`] under the default
/// limits.
pub(crate) fn rich_text_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = crate::types::common_findings(values);
    if let Err(error) = validate(&paragraphs_of(values), &Limits::DEFAULT) {
        findings.push(finding(error));
    }
    findings
}

/// The declarations of the text types and the font table.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (
            TEXT,
            crate::types::with_kind(text_fields()),
            vec![text_findings],
        ),
        (
            RICH_TEXT,
            crate::types::with_kind(rich_text_fields()),
            vec![rich_text_findings],
        ),
        (
            crate::fonts::FONT_TABLE,
            crate::fonts::table_fields(),
            vec![],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_splits_on_every_line_break() {
        let p = paragraphs_from_plain("a\r\nb\rc\n\nd", &Limits::DEFAULT).unwrap();
        let texts: Vec<String> = p
            .iter()
            .map(|p| p.runs.first().map(|r| r.text.clone()).unwrap_or_default())
            .collect();
        assert_eq!(texts, ["a", "b", "c", "", "d"]);
    }

    #[test]
    fn values_round_trip_and_rules_hold() {
        let p = Paragraph {
            alignment: Alignment::Justify,
            tabs: vec![5.0, 10.0],
            runs: vec![
                Run {
                    text: "x\ty".into(),
                    font: Some("f".into()),
                    height: Some(3.0),
                    color: Some([1.0, 0.0, 0.0, 1.0]),
                    bold: true,
                    ..Run::default()
                },
                Run {
                    text: "1".into(),
                    stack: Stack::Horizontal,
                    denominator: Some("2".into()),
                    ..Run::default()
                },
            ],
            ..Paragraph::default()
        };
        assert_eq!(Paragraph::from_value(&p.to_value()).unwrap(), p);
        let limits = Limits::DEFAULT;
        assert!(validate(std::slice::from_ref(&p), &limits).is_ok());
        let code = |p: Paragraph| validate(&[p], &limits).unwrap_err().code();
        let mut bad = p.clone();
        bad.tabs = vec![3.0, 3.0];
        assert_eq!(code(bad), "draft.text.tabs_invalid");
        let mut bad = p.clone();
        bad.runs[1].denominator = None;
        assert_eq!(code(bad), "draft.text.stack_invalid");
        let mut bad = p.clone();
        bad.runs[0].text = "a\nb".into();
        assert_eq!(code(bad), "draft.text.content_invalid");
        let tight = Limits {
            array_length: 1,
            ..Limits::DEFAULT
        };
        assert_eq!(
            validate(&[p.clone(), p], &tight).unwrap_err().code(),
            "draft.text.too_large"
        );
    }
}
