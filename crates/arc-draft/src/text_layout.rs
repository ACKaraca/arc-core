//! Text layout (CORE-DRAFT-034): one function, [`layout`], for viewport, plot and export.
//!
//! The input is the text's data ([`TextInput`]), its resolved style ([`TextStyle`]) and the
//! fonts ([`Fonts`] and the document's substitution table). The output ([`TextLayout`]) is in
//! *text space*: x along the baseline, y up, in drawing units, with the origin at the
//! attachment point. It holds positioned glyph runs (stroke fonts: exact curves; outline fonts:
//! glyph identifiers with positions and size), the line boxes, the overall bounds and every
//! font substitution made. Fonts come only from the provider, and the stroke engine uses only
//! the four arithmetic operations of IEEE 754 on its inputs as long as the width factor is 1
//! and no slant applies (slanted or widened arcs go through the transforms of `arc-geom`), so
//! the same input gives the same output on every platform.
//!
//! The rules (heights, advances, kerning, tabs, line breaking, justification, columns,
//! stacked fractions, decorations, attachment) are stated in the crate specification,
//! section "Text and fonts as implemented".

use std::sync::Arc;

use arc_base::math::{Matrix3, Point2, Transform2, Vector2};
use arc_base::{Color, Error, Result, Tolerance};
use arc_geom::{Arc2, Curve2, Ellipse2, EllipticalArc2, Polyline2, PolylineVertex, Segment2};

use crate::error::codes;
use crate::fonts::{
    FontSource, Fonts, Resolution, SubstitutedBy, Substitution, Substitutions, resolve,
};
use crate::stroke_font::{Element, StrokeFont};
use crate::text::{Alignment, Paragraph, Run, Stack};

/// The slant of italic runs: the tangent of 12 degrees, as a fraction.
pub const ITALIC_SLANT: f64 = 0.2126;
/// The horizontal offset of the second pass of bold stroke runs, as a fraction of the height.
pub const BOLD_OFFSET: f64 = 0.05;
/// The height of the parts of a stacked fraction, as a fraction of the run height.
pub const FRACTION_SCALE: f64 = 0.6;
/// The interval of default tab stops, in text heights.
pub const DEFAULT_TAB: f64 = 4.0;

/// The horizontal anchor of a text block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HAlign {
    /// The left edge.
    #[default]
    Left,
    /// The middle.
    Center,
    /// The right edge.
    Right,
}

/// The vertical anchor of a text block.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VAlign {
    /// The baseline of the first line.
    #[default]
    Baseline,
    /// The bottom of the last line's box.
    Bottom,
    /// Halfway between top and bottom.
    Middle,
    /// The top: the cap height of the first line.
    Top,
}

/// The style values layout uses, already resolved from the text style.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// The base font name; absent is the default stroke font.
    pub font: Option<String>,
    /// The horizontal scale of glyphs, positive.
    pub width_factor: f64,
    /// The slant angle in radians, less than a quarter turn either way.
    pub oblique: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font: None,
            width_factor: 1.0,
            oblique: 0.0,
        }
    }
}

/// Column flow: lines move to the next column when they would reach below `height`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Columns {
    /// The number of columns, at least 1.
    pub count: u32,
    /// The space between columns.
    pub gutter: f64,
    /// The height of a column; 0 keeps one column.
    pub height: f64,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            count: 1,
            gutter: 0.0,
            height: 0.0,
        }
    }
}

/// The text to lay out.
#[derive(Debug, Clone, PartialEq)]
pub struct TextInput<'a> {
    /// The paragraphs.
    pub paragraphs: &'a [Paragraph],
    /// The text height: the cap height of runs without their own height.
    pub height: f64,
    /// The wrapping width; 0 does not wrap.
    pub width: f64,
    /// The horizontal anchor.
    pub halign: HAlign,
    /// The vertical anchor.
    pub valign: VAlign,
    /// The columns.
    pub columns: Columns,
    /// The resolved style.
    pub style: &'a TextStyle,
}

/// The bounds of layout work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// The tolerance of comparisons and transforms.
    pub tolerance: Tolerance,
    /// The most curves and glyphs of one layout; more fail with `draft.display.too_large`.
    pub max_items: usize,
}

/// One glyph of an outline run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    /// The glyph identifier in the font.
    pub id: u32,
    /// The pen position on the baseline, in text space.
    pub position: Point2,
}

/// The kind of a glyph run.
#[derive(Debug, Clone, PartialEq)]
pub enum RunKind {
    /// Stroke text: exact curves in text space (lines, polylines, arcs, elliptical arcs),
    /// including decorations and fraction bars.
    Stroke(Vec<Curve2>),
    /// Outline text for the renderer and plot stream.
    Outline {
        /// The family name the provider registered the font under.
        family: String,
        /// The face index.
        index: u32,
        /// The font size (em size) in drawing units.
        size: f64,
        /// The glyphs.
        glyphs: Vec<PositionedGlyph>,
    },
}

/// A positioned run: the part of one input run on one line.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRun {
    /// The paragraph index.
    pub paragraph: usize,
    /// The run index in the paragraph.
    pub run: usize,
    /// The run's own color; `None` takes the entity's.
    pub color: Option<Color>,
    /// The content.
    pub kind: RunKind,
}

/// One line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineBox {
    /// The paragraph index.
    pub paragraph: usize,
    /// The column index.
    pub column: u32,
    /// The left end of the content.
    pub left: f64,
    /// The right end of the content.
    pub right: f64,
    /// The baseline height.
    pub baseline: f64,
    /// The extent above the baseline: the line height.
    pub ascent: f64,
    /// The extent below the baseline, positive.
    pub descent: f64,
}

/// The result of [`layout`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextLayout {
    /// The runs in paragraph, line and run order.
    pub runs: Vec<GlyphRun>,
    /// The lines in order.
    pub lines: Vec<LineBox>,
    /// The union of the line boxes: lower left and upper right; `None` without lines.
    pub bounds: Option<(Point2, Point2)>,
    /// Every substitution made, without repetition, in order of first use.
    pub substitutions: Vec<Substitution>,
    /// The number of characters the fonts lacked, drawn with the missing glyph.
    pub missing: u64,
}

impl TextLayout {
    pub(crate) fn note(&mut self, substitution: Option<Substitution>) {
        if let Some(s) = substitution
            && !self.substitutions.contains(&s)
        {
            self.substitutions.push(s);
        }
    }
}

pub(crate) fn invalid(name: &str) -> Error {
    Error::new(codes::TEXT_CONTENT_INVALID).with_detail("field", name)
}

pub(crate) fn too_large(max: usize) -> Error {
    Error::new(codes::DISPLAY_TOO_LARGE).with_arg("limit", i64::try_from(max).unwrap_or(i64::MAX))
}

/// Lays out `input` with the fonts of `fonts` and the document table `document`.
///
/// The base font (the style's) decides the engine: a stroke font lays out with the stroke
/// engine of this module; an outline font (with the `outline-text` feature) with the outline
/// engine. A run naming a font of the other kind uses the base font, listed as a substitution
/// by [`SubstitutedBy::Base`].
///
/// # Errors
///
/// `draft.text.content_invalid` (detail `field`) for a height, width, width factor, slant or
/// column value that is not finite or out of range; `draft.display.too_large` beyond
/// `max_items`; `draft.font.default_missing`; `draft.font.outline_invalid` for outline bytes
/// that cannot be read; the `arc-geom` errors of curves too small or too large to represent.
pub fn layout(
    input: &TextInput<'_>,
    document: &Substitutions,
    fonts: &Fonts,
    options: &LayoutOptions,
) -> Result<TextLayout> {
    if !(input.height.is_finite() && input.height > 0.0) {
        return Err(invalid("height"));
    }
    if !(input.width.is_finite() && input.width >= 0.0) {
        return Err(invalid("width"));
    }
    let s = input.style;
    if !(s.width_factor.is_finite() && s.width_factor > 0.0) {
        return Err(invalid("width_factor"));
    }
    if !(s.oblique.is_finite() && s.oblique.abs() < std::f64::consts::FRAC_PI_2) {
        return Err(invalid("oblique"));
    }
    let c = input.columns;
    if c.count == 0
        || !(c.gutter.is_finite() && c.gutter >= 0.0)
        || !(c.height.is_finite() && c.height >= 0.0)
    {
        return Err(invalid("columns"));
    }
    let base = resolve(s.font.as_deref(), document, fonts)?;
    let mut out = TextLayout::default();
    out.note(base.substitution.clone());
    match &base.source {
        FontSource::Stroke(font) => {
            Stroke::new(input, document, fonts, options, &base, Arc::clone(font)).run(&mut out)?;
        }
        FontSource::Outline(_) => outline(input, document, fonts, options, &base, &mut out)?,
    }
    Ok(out)
}

#[cfg(feature = "outline-text")]
fn outline(
    input: &TextInput<'_>,
    document: &Substitutions,
    fonts: &Fonts,
    options: &LayoutOptions,
    base: &Resolution,
    out: &mut TextLayout,
) -> Result<()> {
    crate::outline::layout(input, document, fonts, options, base, out)
}

#[cfg(not(feature = "outline-text"))]
fn outline(
    _: &TextInput<'_>,
    _: &Substitutions,
    _: &Fonts,
    _: &LayoutOptions,
    _: &Resolution,
    _: &mut TextLayout,
) -> Result<()> {
    // Resolution offers outline fonts only with the feature.
    Err(Error::new(codes::FONT_OUTLINE_INVALID))
}

/// The run-level values of the stroke engine.
struct RunMetrics {
    font: Arc<StrokeFont>,
    height: f64,
    /// Font units to drawing units, horizontally and vertically.
    sx: f64,
    sy: f64,
    /// The shear: x grows by `shear` times the height above the baseline.
    shear: f64,
}

#[derive(Debug, Clone, Copy)]
enum Item {
    Glyph { c: char, advance: f64 },
    Space { advance: f64 },
    Tab,
    Fraction { width: f64 },
}

#[derive(Debug, Clone, Copy)]
struct Placed {
    run: usize,
    item: usize,
    x: f64,
    width: f64,
}

/// Where one line goes: its paragraph and column, the alignment region and the offsets.
struct Placement {
    paragraph: usize,
    column: u32,
    region: f64,
    dx: f64,
    baseline: f64,
    descent_ratio: f64,
}

#[derive(Debug, Default)]
struct Line {
    placed: Vec<Placed>,
    end: f64,
    /// Positions in `placed` where a word starts after spaces.
    gaps: Vec<usize>,
    last_tab: Option<usize>,
    height: f64,
    last: bool,
}

struct Stroke<'a> {
    input: &'a TextInput<'a>,
    document: &'a Substitutions,
    fonts: &'a Fonts,
    options: &'a LayoutOptions,
    base: &'a Resolution,
    base_font: Arc<StrokeFont>,
    count: usize,
}

fn advance_of(font: &StrokeFont, c: char) -> (Option<char>, f64) {
    if font.glyph(c).is_some() {
        return (Some(c), font.glyph(c).map_or(0.0, |g| g.advance));
    }
    match font
        .missing
        .and_then(|m| font.glyph(m).map(|g| (m, g.advance)))
    {
        Some((m, advance)) => (Some(m), advance),
        None => (None, font.units * 0.5),
    }
}

impl<'a> Stroke<'a> {
    fn new(
        input: &'a TextInput<'a>,
        document: &'a Substitutions,
        fonts: &'a Fonts,
        options: &'a LayoutOptions,
        base: &'a Resolution,
        base_font: Arc<StrokeFont>,
    ) -> Self {
        Self {
            input,
            document,
            fonts,
            options,
            base,
            base_font,
            count: 0,
        }
    }

    fn metrics(&self, run: &Run, out: &mut TextLayout) -> Result<RunMetrics> {
        let font = match &run.font {
            None => Arc::clone(&self.base_font),
            Some(name) => {
                let r = resolve(Some(name), self.document, self.fonts)?;
                match r.source {
                    FontSource::Stroke(font) => {
                        out.note(r.substitution);
                        font
                    }
                    FontSource::Outline(_) => {
                        out.note(Some(Substitution {
                            requested: name.clone(),
                            used: self.base.name.clone(),
                            by: SubstitutedBy::Base,
                        }));
                        Arc::clone(&self.base_font)
                    }
                }
            }
        };
        let height = run.height.unwrap_or(self.input.height);
        let sy = height / font.cap_height;
        let oblique = self.input.style.oblique;
        // No trigonometry for upright text: tan is not correctly rounded on every platform.
        let slant = if oblique == 0.0 { 0.0 } else { oblique.tan() }
            + if run.italic { ITALIC_SLANT } else { 0.0 };
        Ok(RunMetrics {
            sx: sy * self.input.style.width_factor,
            sy,
            shear: slant,
            height,
            font,
        })
    }

    fn items(&self, run: &Run, m: &RunMetrics, out: &mut TextLayout) -> Vec<Item> {
        if run.stack != Stack::None {
            let width = self.fraction_width(run, m);
            return vec![Item::Fraction { width }];
        }
        let chars: Vec<char> = run.text.chars().collect();
        let mut items = Vec::with_capacity(chars.len());
        for (i, &c) in chars.iter().enumerate() {
            items.push(match c {
                '\t' => Item::Tab,
                ' ' => Item::Space {
                    advance: advance_of(&m.font, ' ').1 * m.sx,
                },
                _ => {
                    let (shown, advance) = advance_of(&m.font, c);
                    if shown != Some(c) {
                        out.missing = out.missing.saturating_add(1);
                    }
                    let kern = chars.get(i + 1).map_or(0.0, |next| m.font.kern(c, *next));
                    Item::Glyph {
                        c,
                        advance: (advance + kern) * m.sx,
                    }
                }
            });
        }
        items
    }

    fn text_width(font: &StrokeFont, text: &str, sx: f64) -> f64 {
        let chars: Vec<char> = text.chars().collect();
        chars
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let kern = chars.get(i + 1).map_or(0.0, |n| font.kern(*c, *n));
                (advance_of(font, *c).1 + kern) * sx
            })
            .sum()
    }

    fn fraction_width(&self, run: &Run, m: &RunMetrics) -> f64 {
        let sx = m.sx * FRACTION_SCALE;
        let n = Self::text_width(&m.font, &run.text, sx);
        let d = Self::text_width(&m.font, run.denominator.as_deref().unwrap_or(""), sx);
        match run.stack {
            Stack::Diagonal => n + 0.4 * m.height + d,
            _ => n.max(d) + 0.2 * m.height,
        }
    }

    fn next_tab(&self, paragraph: &Paragraph, x: f64) -> f64 {
        let tol = self.options.tolerance.linear;
        if let Some(t) = paragraph.tabs.iter().find(|t| **t > x + tol) {
            return *t;
        }
        let interval = DEFAULT_TAB * self.input.height;
        ((x + tol) / interval).floor().mul_add(interval, interval)
    }

    /// Breaks one paragraph into lines.
    #[expect(clippy::too_many_lines, reason = "one greedy pass over the words")]
    fn lines(&self, paragraph: &Paragraph, items: &[Vec<Item>], heights: &[f64]) -> Vec<Line> {
        let tol = self.options.tolerance.linear;
        let width = self.input.width;
        // Tokens: (is_space, run, item) in order.
        let flat: Vec<(usize, usize, Item)> = items
            .iter()
            .enumerate()
            .flat_map(|(r, list)| list.iter().enumerate().map(move |(i, it)| (r, i, *it)))
            .collect();
        let empty_height = heights.first().copied().unwrap_or(self.input.height);
        let new_line = |first: bool| -> (Line, f64) {
            (
                Line {
                    height: 0.0,
                    ..Line::default()
                },
                if first {
                    paragraph.indent_first
                } else {
                    paragraph.indent
                },
            )
        };
        let mut lines = Vec::new();
        let (mut line, mut x) = new_line(true);
        let mut pending: Vec<(usize, usize, f64)> = Vec::new();
        let mut i = 0;
        while i < flat.len() {
            let Some(&(run, item, it)) = flat.get(i) else {
                break;
            };
            if let Item::Space { advance } = it {
                pending.push((run, item, advance));
                i += 1;
                continue;
            }
            let mut j = i;
            while let Some((_, _, w)) = flat.get(j) {
                if matches!(w, Item::Space { .. }) {
                    break;
                }
                j += 1;
            }
            let word = flat.get(i..j).unwrap_or(&[]);
            let start = x + pending.iter().map(|p| p.2).sum::<f64>();
            let mut end = self.simulate(paragraph, word, start);
            let mut at = start;
            if width > 0.0 && end > width + tol && !line.placed.is_empty() {
                lines.push(std::mem::take(&mut line));
                let (fresh, indent) = new_line(false);
                line = fresh;
                pending.clear();
                at = indent;
                end = self.simulate(paragraph, word, at);
            } else if !line.placed.is_empty() && !pending.is_empty() {
                let mut px = x;
                for (run, item, advance) in pending.drain(..) {
                    line.placed.push(Placed {
                        run,
                        item,
                        x: px,
                        width: advance,
                    });
                    px += advance;
                }
                line.gaps.push(line.placed.len());
            } else {
                // Leading spaces of a paragraph keep their room.
                let mut px = x;
                for (run, item, advance) in pending.drain(..) {
                    line.placed.push(Placed {
                        run,
                        item,
                        x: px,
                        width: advance,
                    });
                    px += advance;
                }
            }
            let mut wx = at;
            for &(run, item, it) in word {
                let w = match it {
                    Item::Glyph { advance, .. } | Item::Space { advance } => advance,
                    Item::Fraction { width } => width,
                    Item::Tab => {
                        line.last_tab = Some(line.placed.len());
                        self.next_tab(paragraph, wx) - wx
                    }
                };
                line.placed.push(Placed {
                    run,
                    item,
                    x: wx,
                    width: w,
                });
                line.height = line.height.max(heights.get(run).copied().unwrap_or(0.0));
                wx += w;
            }
            line.end = end;
            x = end;
            i = j;
        }
        if line.height == 0.0 {
            line.height = empty_height;
        }
        line.last = true;
        lines.push(line);
        for l in &mut lines {
            if l.height == 0.0 {
                l.height = empty_height;
            }
        }
        lines
    }

    fn simulate(&self, paragraph: &Paragraph, word: &[(usize, usize, Item)], start: f64) -> f64 {
        word.iter().fold(start, |x, (_, _, it)| match it {
            Item::Glyph { advance, .. } | Item::Space { advance } => x + advance,
            Item::Fraction { width } => x + width,
            Item::Tab => self.next_tab(paragraph, x),
        })
    }

    fn run(mut self, out: &mut TextLayout) -> Result<()> {
        let input = self.input;
        let tol = self.options.tolerance.linear;
        // Per paragraph: run metrics, items, lines.
        let mut prepared = Vec::with_capacity(input.paragraphs.len());
        for paragraph in input.paragraphs {
            let mut metrics = Vec::with_capacity(paragraph.runs.len());
            let mut items = Vec::with_capacity(paragraph.runs.len());
            for run in &paragraph.runs {
                let m = self.metrics(run, out)?;
                items.push(self.items(run, &m, out));
                metrics.push(m);
            }
            let heights: Vec<f64> = metrics.iter().map(|m| m.height).collect();
            let lines = self.lines(paragraph, &items, &heights);
            prepared.push((metrics, items, lines));
        }
        let max_end = prepared
            .iter()
            .flat_map(|(_, _, lines)| lines.iter().map(|l| l.end))
            .fold(0.0, f64::max);
        let region = if input.width > 0.0 {
            input.width
        } else {
            max_end
        };
        let font = Arc::clone(&self.base_font);
        let pitch_ratio = font.line_spacing / font.cap_height;
        let descent_ratio = -font.descender / font.cap_height;
        let flow = input.columns.count > 1 && input.columns.height > 0.0 && input.width > 0.0;
        let mut top = 0.0;
        let mut column = 0_u32;
        let mut column_lines = 0_usize;
        let mut placed_lines = Vec::new();
        for (pi, (paragraph, (_, _, lines))) in input.paragraphs.iter().zip(&prepared).enumerate() {
            top -= paragraph.space_before;
            for (li, line) in lines.iter().enumerate() {
                let descent = line.height * descent_ratio;
                if flow
                    && column_lines > 0
                    && top - line.height - descent < -input.columns.height - tol
                    && column + 1 < input.columns.count
                {
                    column += 1;
                    column_lines = 0;
                    top = 0.0;
                }
                let baseline = top - line.height;
                let offset = f64::from(column) * (input.width + input.columns.gutter);
                placed_lines.push((pi, li, baseline, offset, column));
                column_lines += 1;
                top -= line.height * pitch_ratio * paragraph.line_spacing;
            }
            top -= paragraph.space_after;
        }
        // Anchor.
        let columns_used = placed_lines.iter().map(|l| l.4).max().unwrap_or(0) + 1;
        let block_right = if flow {
            f64::from(columns_used) * (input.width + input.columns.gutter) - input.columns.gutter
        } else {
            region
        };
        let bottom = placed_lines
            .iter()
            .filter_map(|(pi, li, baseline, _, _)| {
                let line = prepared.get(*pi)?.2.get(*li)?;
                Some(baseline - line.height * descent_ratio)
            })
            .fold(0.0, f64::min);
        let first_baseline = placed_lines.first().map_or(0.0, |l| l.2);
        let ax = match input.halign {
            HAlign::Left => 0.0,
            HAlign::Center => block_right * 0.5,
            HAlign::Right => block_right,
        };
        let ay = match input.valign {
            VAlign::Baseline => first_baseline,
            VAlign::Bottom => bottom,
            VAlign::Middle => bottom * 0.5,
            VAlign::Top => 0.0,
        };
        for (pi, li, baseline, offset, column) in placed_lines {
            let (Some(paragraph), Some((metrics, items, lines))) =
                (input.paragraphs.get(pi), prepared.get(pi))
            else {
                continue;
            };
            let Some(line) = lines.get(li) else {
                continue;
            };
            let at = Placement {
                paragraph: pi,
                column,
                region,
                dx: offset - ax,
                baseline: baseline - ay,
                descent_ratio,
            };
            self.emit(paragraph, metrics, items, line, &at, out)?;
        }
        out.bounds = bounds(&out.lines);
        Ok(())
    }

    fn emit(
        &mut self,
        paragraph: &Paragraph,
        metrics: &[RunMetrics],
        items: &[Vec<Item>],
        line: &Line,
        at: &Placement,
        out: &mut TextLayout,
    ) -> Result<()> {
        let input = self.input;
        let (pi, region, by) = (at.paragraph, at.region, at.baseline);
        let free = region - line.end;
        let eligible: Vec<usize> = line
            .gaps
            .iter()
            .copied()
            .filter(|g| line.last_tab.is_none_or(|t| *g > t))
            .collect();
        let justify = paragraph.alignment == Alignment::Justify
            && !line.last
            && input.width > 0.0
            && free > 0.0
            && !eligible.is_empty();
        let shift = match paragraph.alignment {
            Alignment::Center => free * 0.5,
            Alignment::Right => free,
            Alignment::Left | Alignment::Justify => 0.0,
        };
        let per_gap = if justify {
            #[expect(clippy::cast_precision_loss, reason = "gap counts are small")]
            let n = eligible.len() as f64;
            free / n
        } else {
            0.0
        };
        let dx = at.dx + shift;
        let x_of = |k: usize, p: &Placed| {
            let gaps = eligible.iter().filter(|g| **g <= k).count();
            #[expect(clippy::cast_precision_loss, reason = "gap counts are small")]
            let extra = per_gap * gaps as f64;
            p.x + extra + dx
        };
        let content: Vec<(usize, &Placed)> = line.placed.iter().enumerate().collect();
        let left = line
            .placed
            .first()
            .map_or(dx + paragraph.indent, |p| x_of(0, p));
        let right = line
            .placed
            .last()
            .map_or(left, |p| x_of(line.placed.len() - 1, p) + p.width);
        out.lines.push(LineBox {
            paragraph: pi,
            column: at.column,
            left,
            right,
            baseline: by,
            ascent: line.height,
            descent: line.height * at.descent_ratio,
        });
        // One glyph run per stretch of one input run on this line.
        let mut k = 0;
        while k < content.len() {
            let Some(&(_, first)) = content.get(k) else {
                break;
            };
            let run_index = first.run;
            let mut curves = Vec::new();
            let (mut x0, mut x1) = (f64::INFINITY, f64::NEG_INFINITY);
            while let Some(&(pos, p)) = content.get(k) {
                if p.run != run_index {
                    break;
                }
                let x = x_of(pos, p);
                x0 = x0.min(x);
                x1 = x1.max(x + p.width);
                if let (Some(m), Some(item), Some(run)) = (
                    metrics.get(run_index),
                    items.get(run_index).and_then(|l| l.get(p.item)),
                    paragraph.runs.get(run_index),
                ) {
                    self.item_curves(*item, run, m, Point2::new(x, by), &mut curves)?;
                }
                k += 1;
            }
            if let (Some(m), Some(run)) = (metrics.get(run_index), paragraph.runs.get(run_index))
                && x1 > x0
            {
                for (on, at) in [
                    (run.underline, -0.15),
                    (run.overline, 1.2),
                    (run.strike, 0.5),
                ] {
                    if on {
                        let y = by + at * m.height;
                        self.push(&mut curves, segment(x0, y, x1, y)?)?;
                    }
                }
            }
            out.runs.push(GlyphRun {
                paragraph: pi,
                run: run_index,
                color: run_color(paragraph, run_index),
                kind: RunKind::Stroke(curves),
            });
        }
        Ok(())
    }

    fn push(&mut self, curves: &mut Vec<Curve2>, curve: Curve2) -> Result<()> {
        self.count += 1;
        if self.count > self.options.max_items {
            return Err(too_large(self.options.max_items));
        }
        curves.push(curve);
        Ok(())
    }

    fn item_curves(
        &mut self,
        item: Item,
        run: &Run,
        m: &RunMetrics,
        origin: Point2,
        curves: &mut Vec<Curve2>,
    ) -> Result<()> {
        match item {
            Item::Glyph { c, .. } => {
                let passes: &[f64] = if run.bold {
                    &[0.0, BOLD_OFFSET]
                } else {
                    &[0.0]
                };
                for pass in passes {
                    let at = Point2::new(origin.x + pass * m.height, origin.y);
                    self.glyph(&m.font, c, m.sx, m.sy, m.shear, at, curves)?;
                }
                Ok(())
            }
            Item::Fraction { width } => self.fraction(run, m, origin, width, curves),
            Item::Space { .. } | Item::Tab => Ok(()),
        }
    }

    fn fraction(
        &mut self,
        run: &Run,
        m: &RunMetrics,
        origin: Point2,
        width: f64,
        curves: &mut Vec<Curve2>,
    ) -> Result<()> {
        let (sx, sy) = (m.sx * FRACTION_SCALE, m.sy * FRACTION_SCALE);
        let h = m.height;
        let numerator = run.text.as_str();
        let denominator = run.denominator.as_deref().unwrap_or("");
        let wn = Self::text_width(&m.font, numerator, sx);
        let wd = Self::text_width(&m.font, denominator, sx);
        let (n_at, d_at) = match run.stack {
            Stack::Diagonal => {
                let slash_x = origin.x + wn;
                self.push(
                    curves,
                    segment(
                        slash_x,
                        origin.y - 0.1 * h,
                        slash_x + 0.4 * h,
                        origin.y + 1.1 * h,
                    )?,
                )?;
                (
                    Point2::new(origin.x, origin.y + 0.4 * h),
                    Point2::new(slash_x + 0.4 * h, origin.y),
                )
            }
            _ => {
                let bar = origin.y + 0.5 * h;
                self.push(curves, segment(origin.x, bar, origin.x + width, bar)?)?;
                (
                    Point2::new(origin.x + (width - wn) * 0.5, bar + 0.15 * h),
                    Point2::new(
                        origin.x + (width - wd) * 0.5,
                        bar - 0.15 * h - FRACTION_SCALE * h,
                    ),
                )
            }
        };
        for (text, at) in [(numerator, n_at), (denominator, d_at)] {
            let mut x = at.x;
            let chars: Vec<char> = text.chars().collect();
            for (i, c) in chars.iter().enumerate() {
                self.glyph(&m.font, *c, sx, sy, m.shear, Point2::new(x, at.y), curves)?;
                let kern = chars.get(i + 1).map_or(0.0, |n| m.font.kern(*c, *n));
                x += (advance_of(&m.font, *c).1 + kern) * sx;
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_arguments, reason = "the glyph placement values")]
    fn glyph(
        &mut self,
        font: &StrokeFont,
        c: char,
        sx: f64,
        sy: f64,
        shear: f64,
        at: Point2,
        curves: &mut Vec<Curve2>,
    ) -> Result<()> {
        let (Some(shown), _) = advance_of(font, c) else {
            return Ok(());
        };
        let Some(glyph) = font.glyph(shown) else {
            return Ok(());
        };
        let map = |p: &Point2| Point2::new(at.x + sx * p.x + shear * (sy * p.y), at.y + sy * p.y);
        for element in &glyph.elements {
            let curve = match element {
                Element::Polyline(points) => {
                    let vertices = points
                        .iter()
                        .map(|p| PolylineVertex::new(map(p), 0.0))
                        .collect();
                    Curve2::Polyline(Polyline2::new(vertices, false)?)
                }
                Element::Arc {
                    center,
                    radius,
                    start,
                    sweep,
                } => {
                    let (start, sweep) = (start.to_radians(), sweep.to_radians());
                    if shear != 0.0 {
                        let t = Transform2::from_matrix_unchecked(Matrix3::new(
                            sx,
                            shear * sy,
                            at.x,
                            0.0,
                            sy,
                            at.y,
                            0.0,
                            0.0,
                            1.0,
                        ));
                        Curve2::Arc(Arc2::new(*center, *radius, start, sweep)?)
                            .transformed(&t, &self.options.tolerance)?
                    } else if sx == sy {
                        Curve2::Arc(Arc2::new(map(center), radius * sy, start, sweep)?)
                    } else if sx > sy {
                        let ellipse =
                            Ellipse2::new(map(center), Vector2::new(radius * sx, 0.0), sy / sx)?;
                        Curve2::EllipticalArc(EllipticalArc2::new(ellipse, start, sweep)?)
                    } else {
                        let ellipse =
                            Ellipse2::new(map(center), Vector2::new(0.0, radius * sy), sx / sy)?;
                        Curve2::EllipticalArc(EllipticalArc2::new(
                            ellipse,
                            start - std::f64::consts::FRAC_PI_2,
                            sweep,
                        )?)
                    }
                }
            };
            self.push(curves, curve)?;
        }
        Ok(())
    }
}

fn run_color(paragraph: &Paragraph, run: usize) -> Option<Color> {
    paragraph
        .runs
        .get(run)
        .and_then(|r| r.color)
        .map(|[r, g, b, a]| Color::new(r, g, b, a))
}

fn segment(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<Curve2> {
    Ok(Curve2::Segment(Segment2::new(
        Point2::new(x0, y0),
        Point2::new(x1, y1),
    )?))
}

pub(crate) fn bounds(lines: &[LineBox]) -> Option<(Point2, Point2)> {
    lines.iter().fold(None, |acc, l| {
        let lo = Point2::new(l.left.min(l.right), l.baseline - l.descent);
        let hi = Point2::new(l.right.max(l.left), l.baseline + l.ascent);
        Some(match acc {
            None => (lo, hi),
            Some((a, b)) => (
                Point2::new(a.x.min(lo.x), a.y.min(lo.y)),
                Point2::new(b.x.max(hi.x), b.y.max(hi.y)),
            ),
        })
    })
}
