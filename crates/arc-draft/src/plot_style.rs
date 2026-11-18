//! Plot style tables (CORE-DRAFT-030).
//!
//! A plot style table is a `core.draft.plot_style_table` object of one of two kinds. A
//! **color-mapped** table holds at most one style per indexed color (0 to 255): a primitive
//! whose color has the source `Indexed(i)` takes the style of index `i`. A **named** table holds
//! styles by name: a primitive takes the style whose name is the `plot_style` field of its
//! drawing entity, or of the entity's layer when the entity has none. A primitive without a
//! matching style keeps its own appearance.
//!
//! [`resolve_pen`] is a pure function from a table and an [`Appearance`] to a [`Pen`].

use arc_base::{Color, Error, Limits, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{
    Bound, ColorSource, ColorValue, FieldDecl, Finding, OnDelete, Strength, TypeValidator, Value,
    ValueType,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{Created, Delete, create, delete, family, spec};
use crate::error::codes;
use crate::types::{
    LINETYPE, MAX_LINEWEIGHT_MM, NAME_BYTES, TEXT_BYTES, enum_at, f64_at, finding, name_field,
    real, reference, reference_at, text, text_at, variants,
};

/// The type key of a plot style table.
pub const PLOT_STYLE_TABLE: &str = "core.draft.plot_style_table";

/// The most styles of one table.
pub const MAX_STYLES: u64 = 4096;

/// The lineweight in millimeters of a stroke whose resolved lineweight is the default.
pub const DEFAULT_LINEWEIGHT_MM: f64 = 0.25;

/// Field indices of a table.
pub mod table {
    /// `name`.
    pub const NAME: usize = 0;
    /// `kind`: `color_mapped` or `named`.
    pub const KIND: usize = 1;
    /// `description`.
    pub const DESCRIPTION: usize = 2;
    /// `styles`: the records of [`super::style`].
    pub const STYLES: usize = 3;
}

/// Field indices of a style record.
pub mod style {
    /// `name`: the key of a style in a named table.
    pub const NAME: usize = 0;
    /// `index`: the key of a style in a color-mapped table, 0 to 255.
    pub const INDEX: usize = 1;
    /// `color`: the pen color; absent uses the object's color.
    pub const COLOR: usize = 2;
    /// `lineweight`: the pen width in millimeters; absent uses the object's lineweight.
    pub const LINEWEIGHT: usize = 3;
    /// `screening`: ink percentage, 0 to 100.
    pub const SCREENING: usize = 4;
    /// `linetype`: the linetype that replaces the object's; absent uses the object's.
    pub const LINETYPE: usize = 5;
    /// `end`: the end style of strokes.
    pub const END: usize = 6;
    /// `join`: the join style of strokes.
    pub const JOIN: usize = 7;
}

/// The kind of a table, with its enum code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    /// One style per indexed color.
    ColorMapped = 0,
    /// Styles by name.
    Named = 1,
}

/// How the end of an open stroke or dash is drawn, with its enum code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndStyle {
    /// Flat, at the end point.
    #[default]
    Butt = 0,
    /// Flat, half the width beyond the end point.
    Square = 1,
    /// A half disc of the pen width.
    Round = 2,
}

/// How a corner of a stroke is drawn, with its enum code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JoinStyle {
    /// The outer edges extend to a point (up to a miter limit of 4).
    #[default]
    Miter = 0,
    /// The corner is cut off.
    Bevel = 1,
    /// The corner is rounded.
    Round = 2,
}

impl EndStyle {
    /// The style of an enum code; unknown codes read as [`EndStyle::Butt`].
    #[must_use]
    pub fn of(code: Option<u32>) -> Self {
        match code {
            Some(1) => Self::Square,
            Some(2) => Self::Round,
            _ => Self::Butt,
        }
    }
}

impl JoinStyle {
    /// The style of an enum code; unknown codes read as [`JoinStyle::Miter`].
    #[must_use]
    pub fn of(code: Option<u32>) -> Self {
        match code {
            Some(1) => Self::Bevel,
            Some(2) => Self::Round,
            _ => Self::Miter,
        }
    }
}

/// One style: how a primitive plots.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotStyle {
    /// The pen color; `None` uses the object's color.
    pub color: Option<Color>,
    /// The pen width in millimeters; `None` uses the object's lineweight.
    pub lineweight: Option<f64>,
    /// The ink percentage, 0 to 100.
    pub screening: f64,
    /// The replacement linetype; `None` uses the object's.
    pub linetype: Option<ObjectId>,
    /// The end style.
    pub end: EndStyle,
    /// The join style.
    pub join: JoinStyle,
}

impl Default for PlotStyle {
    fn default() -> Self {
        Self {
            color: None,
            lineweight: None,
            screening: 100.0,
            linetype: None,
            end: EndStyle::Butt,
            join: JoinStyle::Miter,
        }
    }
}

/// A style with its key: the name in a named table, the index in a color-mapped one.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The name; `None` when the record has none.
    pub name: Option<String>,
    /// The color index; `None` when the record has none.
    pub index: Option<u8>,
    /// The style.
    pub style: PlotStyle,
}

/// A plot style table read from its object.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotStyleTable {
    /// The kind.
    pub kind: TableKind,
    /// The styles in stored order; for a repeated key the first one wins.
    pub entries: Vec<Entry>,
}

impl PlotStyleTable {
    /// The table of the values of a `core.draft.plot_style_table` object. Records that do not
    /// have the shape of the schema are skipped.
    #[must_use]
    pub fn from_values(values: &[Option<Value>]) -> Self {
        let kind = if enum_at(values, table::KIND) == Some(1) {
            TableKind::Named
        } else {
            TableKind::ColorMapped
        };
        let entries = match values.get(table::STYLES) {
            Some(Some(Value::List(records))) => records.iter().filter_map(entry).collect(),
            _ => Vec::new(),
        };
        Self { kind, entries }
    }

    /// The style for a primitive: by `index` in a color-mapped table, by `name` in a named one.
    #[must_use]
    pub fn style_for(&self, index: Option<u8>, name: Option<&str>) -> Option<&PlotStyle> {
        self.entries
            .iter()
            .find(|e| match self.kind {
                TableKind::ColorMapped => index.is_some() && e.index == index,
                TableKind::Named => name.is_some() && e.name.as_deref() == name,
            })
            .map(|e| &e.style)
    }
}

fn entry(record: &Value) -> Option<Entry> {
    let Value::Record(f) = record else {
        return None;
    };
    let color = match f.get(style::COLOR) {
        Some(Some(Value::Color(c))) => Some(c.rgba),
        _ => None,
    };
    let index = match f.get(style::INDEX) {
        Some(Some(Value::I64(i))) => u8::try_from(*i).ok(),
        _ => None,
    };
    Some(Entry {
        name: text_at(f, style::NAME).map(str::to_owned),
        index,
        style: PlotStyle {
            color,
            lineweight: f64_at(f, style::LINEWEIGHT),
            screening: f64_at(f, style::SCREENING).unwrap_or(100.0),
            linetype: reference_at(f, style::LINETYPE),
            end: EndStyle::of(enum_at(f, style::END)),
            join: JoinStyle::of(enum_at(f, style::JOIN)),
        },
    })
}

/// The appearance of a primitive as the plot pipeline sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance<'a> {
    /// The resolved color; its source index selects the style of a color-mapped table.
    pub color: &'a ColorValue,
    /// The resolved lineweight in millimeters; `None` is the default lineweight.
    pub lineweight: Option<f64>,
    /// The resolved linetype; `None` is continuous.
    pub linetype: Option<ObjectId>,
    /// The resolved transparency, 0 to 1.
    pub transparency: f64,
    /// The plot style name of the drawing entity or its layer.
    pub style_name: Option<&'a str>,
}

/// A resolved pen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pen {
    /// The linear color after screening; the alpha is the color's alpha times
    /// `1 − transparency`.
    pub color: Color,
    /// The width in millimeters; 0 is the thinnest line the output can draw.
    pub width: f64,
    /// The end style.
    pub cap: EndStyle,
    /// The join style.
    pub join: JoinStyle,
    /// The linetype to draw with; `None` is continuous.
    pub linetype: Option<ObjectId>,
    /// Whether the style replaced the object's linetype, so dashes are generated again.
    pub linetype_overridden: bool,
}

/// The pen of `appearance` under `table` (`None`: plot styles off, or no table).
///
/// 1. The style is looked up as described in the [module documentation](self); without one
///    every value is the object's, the ink is 100 percent, the end style is `butt` and the join
///    style `miter`.
/// 2. Color: the style's, else the object's. Lineweight: the style's, else the object's, else
///    [`DEFAULT_LINEWEIGHT_MM`]. Linetype: the style's, else the object's.
/// 3. Screening `s` percent mixes each linear color channel with white:
///    `c·s/100 + (1 − s/100)`. The alpha of the chosen color is untouched by screening and is
///    multiplied by `1 − transparency`.
#[must_use]
pub fn resolve_pen(table: Option<&PlotStyleTable>, appearance: &Appearance<'_>) -> Pen {
    let index = match &appearance.color.source {
        Some(ColorSource::Indexed(i)) => Some(*i),
        _ => None,
    };
    let style = table.and_then(|t| t.style_for(index, appearance.style_name));
    let default = PlotStyle::default();
    let style_ref = style.unwrap_or(&default);
    let base = style_ref.color.unwrap_or(appearance.color.rgba);
    let ink = (style_ref.screening / 100.0).clamp(0.0, 1.0);
    let mix = |c: f32| {
        #[expect(clippy::cast_possible_truncation, reason = "colors are stored as f32")]
        let ink = ink as f32;
        c.mul_add(ink, 1.0 - ink)
    };
    #[expect(clippy::cast_possible_truncation, reason = "colors are stored as f32")]
    let opacity = (1.0 - appearance.transparency).clamp(0.0, 1.0) as f32;
    Pen {
        color: Color::new(mix(base.r), mix(base.g), mix(base.b), base.a * opacity),
        width: style_ref
            .lineweight
            .or(appearance.lineweight)
            .unwrap_or(DEFAULT_LINEWEIGHT_MM),
        cap: style_ref.end,
        join: style_ref.join,
        linetype: style_ref.linetype.or(appearance.linetype),
        linetype_overridden: style_ref.linetype.is_some(),
    }
}

fn style_record() -> ValueType {
    ValueType::Record(vec![
        text("name", NAME_BYTES),
        FieldDecl::new("index", ValueType::I64)
            .with_bound(Bound::Int { min: 0, max: 255 })
            .optional(),
        FieldDecl::new("color", ValueType::Color).optional(),
        FieldDecl::new("lineweight", ValueType::F64)
            .with_bound(Bound::Real {
                min: 0.0,
                max: MAX_LINEWEIGHT_MM,
            })
            .optional(),
        real("screening", 100.0, 0.0, 100.0),
        FieldDecl::new(
            "linetype",
            reference(LINETYPE, Strength::Associative, OnDelete::Block, true),
        )
        .optional(),
        FieldDecl::new("end", variants(&["butt", "square", "round"])).with_default(Value::Enum(0)),
        FieldDecl::new("join", variants(&["miter", "bevel", "round"])).with_default(Value::Enum(0)),
    ])
}

fn fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        FieldDecl::new("kind", variants(&["color_mapped", "named"])).with_default(Value::Enum(0)),
        text("description", TEXT_BYTES),
        FieldDecl::new("styles", ValueType::List(Box::new(style_record())))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_STYLES }),
    ]
}

/// The findings of a table: a style without the key its kind needs, and a repeated key.
fn findings(values: &[Option<Value>]) -> Vec<Finding> {
    let parsed = PlotStyleTable::from_values(values);
    let mut findings = Vec::new();
    let mut keys: Vec<(Option<u8>, Option<&str>)> = Vec::new();
    for e in &parsed.entries {
        let key = match parsed.kind {
            TableKind::ColorMapped => (e.index, None),
            TableKind::Named => (None, e.name.as_deref().filter(|n| !n.is_empty())),
        };
        if key == (None, None) {
            findings.push(finding(Error::new(codes::PLOT_STYLE_ENTRY_INVALID)));
        } else if keys.contains(&key) {
            let detail = key.1.map_or_else(
                || key.0.map_or_else(String::new, |i| i.to_string()),
                str::to_owned,
            );
            findings.push(finding(
                Error::new(codes::PLOT_STYLE_DUPLICATE).with_arg("key", detail),
            ));
        } else {
            keys.push(key);
        }
    }
    findings
}

/// The declaration of the table type.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![(PLOT_STYLE_TABLE, fields(), vec![findings])]
}

/// One style of `core.draft.plot_style_table.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleParam {
    /// The name, in a named table.
    #[serde(default)]
    #[schemars(length(max = 255))]
    pub name: Option<String>,
    /// The color index (0 to 255), in a color-mapped table.
    #[serde(default)]
    pub index: Option<u8>,
    /// The linear RGBA pen color; absent uses the object's color.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// The pen width in millimeters; absent uses the object's lineweight.
    #[serde(default)]
    pub lineweight: Option<f64>,
    /// The ink percentage, 0 to 100; default 100.
    #[serde(default)]
    pub screening: Option<f64>,
    /// The replacement linetype; absent uses the object's.
    #[serde(default)]
    pub linetype: Option<IdParam>,
    /// The end style; default `butt`.
    #[serde(default)]
    pub end: Option<EndStyle>,
    /// The join style; default `miter`.
    #[serde(default)]
    pub join: Option<JoinStyle>,
}

/// Parameters of `core.draft.plot_style_table.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTable {
    /// The name, unique among plot style tables.
    #[schemars(length(max = 255))]
    pub name: String,
    /// The kind.
    pub kind: TableKind,
    /// The styles.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub styles: Vec<StyleParam>,
}

fn record(p: StyleParam) -> Value {
    let color = p.color.map(|[r, g, b, a]| {
        Value::Color(Box::new(ColorValue {
            rgba: Color::new(r, g, b, a),
            source: None,
        }))
    });
    Value::Record(vec![
        p.name.map(Value::String),
        p.index.map(|i| Value::I64(i64::from(i))),
        color,
        p.lineweight.map(Value::F64),
        Some(Value::F64(p.screening.unwrap_or(100.0))),
        p.linetype.map(|IdParam(l)| Value::Reference(l)),
        Some(Value::Enum(p.end.unwrap_or_default() as u32)),
        Some(Value::Enum(p.join.unwrap_or_default() as u32)),
    ])
}

fn create_table(ctx: &mut Context<'_>, p: CreateTable) -> Result<Created> {
    if p.styles.len() > usize::try_from(Limits::DEFAULT.array_length).unwrap_or(usize::MAX) {
        return Err(crate::commands::field_invalid("styles"));
    }
    let values = vec![
        Some(Value::String(p.name)),
        Some(Value::Enum(p.kind as u32)),
        None,
        Some(Value::List(p.styles.into_iter().map(record).collect())),
    ];
    Ok(Created {
        id: IdParam(create(ctx, PLOT_STYLE_TABLE, None, values)?),
    })
}

/// The commands of plot style tables.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.plot_style_table.create", create_table),
        family("core.draft.plot_style_table.modify", &[PLOT_STYLE_TABLE]),
        spec("core.draft.plot_style_table.delete", |ctx, p: Delete| {
            delete(ctx, p, &[PLOT_STYLE_TABLE])
        }),
    ]
}
