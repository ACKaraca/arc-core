//! The outline engine of text layout (feature `outline-text`): shaping, bidirectional
//! reordering, line breaking and alignment of outline fonts with `parley`.
//!
//! Each call builds a font collection that holds only the provider's fonts named by the text,
//! registered under their provider family names; the system's fonts are never consulted. Each
//! paragraph is one `parley` layout: runs become style ranges (family, size, weight, slant),
//! lines break at the wrapping width and align as the paragraph says. The output is glyph runs
//! (family, face index, size, glyph identifiers and pen positions) in the text space of
//! [`crate::text_layout`]; decorations are stroke segments.
//!
//! The size of an outline run is its height used as the em size. Stacked fractions are shaped
//! as `numerator/denominator`; columns and the paragraph's own tab stops are not applied.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::math::Point2;
use arc_base::{Color, Error, Result};
use arc_geom::{Curve2, Segment2};
use parley::fontique::{Blob, Collection, CollectionOptions, FontInfoOverride, SourceCache};
use parley::style::{FontFamily, FontFamilyName, FontStyle, FontWeight, StyleProperty};
use parley::{
    Alignment as Align, AlignmentOptions, FontContext, LayoutContext, PositionedLayoutItem,
};

use crate::error::codes;
use crate::fonts::{
    FontSource, Fonts, Resolution, SubstitutedBy, Substitution, Substitutions, resolve,
};
use crate::text::{Alignment, Stack};
use crate::text_layout::{
    GlyphRun, HAlign, LayoutOptions, LineBox, PositionedGlyph, RunKind, TextInput, TextLayout,
    VAlign, bounds, too_large,
};

#[expect(clippy::cast_possible_truncation, reason = "parley works in f32")]
fn f32_of(x: f64) -> f32 {
    x as f32
}

/// Lays out `input` whose base font `base` is an outline font.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over paragraphs, lines and runs"
)]
pub(crate) fn layout(
    input: &TextInput<'_>,
    document: &Substitutions,
    fonts: &Fonts,
    options: &LayoutOptions,
    base: &Resolution,
    out: &mut TextLayout,
) -> Result<()> {
    let FontSource::Outline(base_font) = &base.source else {
        return Err(Error::new(codes::FONT_OUTLINE_INVALID));
    };
    // The family of every run, and the fonts to register.
    let mut registered: BTreeMap<String, crate::fonts::OutlineFont> = BTreeMap::new();
    registered.insert(base_font.family.clone(), base_font.clone());
    let mut families: Vec<Vec<String>> = Vec::new();
    for paragraph in input.paragraphs {
        let mut list = Vec::new();
        for run in &paragraph.runs {
            let family = match &run.font {
                None => base_font.family.clone(),
                Some(name) => {
                    let r = resolve(Some(name), document, fonts)?;
                    match r.source {
                        FontSource::Outline(font) => {
                            out.note(r.substitution);
                            let family = font.family.clone();
                            registered.insert(family.clone(), font);
                            family
                        }
                        FontSource::Stroke(_) => {
                            out.note(Some(Substitution {
                                requested: name.clone(),
                                used: base.name.clone(),
                                by: SubstitutedBy::Base,
                            }));
                            base_font.family.clone()
                        }
                    }
                }
            };
            list.push(family);
        }
        families.push(list);
    }
    let mut collection = Collection::new(CollectionOptions {
        shared: false,
        system_fonts: false,
    });
    let mut faces: BTreeMap<u64, crate::fonts::OutlineFont> = BTreeMap::new();
    for font in registered.values() {
        let blob = Blob::new(Arc::new(Arc::clone(&font.data)));
        let found = collection.register_fonts(
            blob.clone(),
            Some(FontInfoOverride {
                family_name: Some(&font.family),
                ..FontInfoOverride::default()
            }),
        );
        if found.is_empty() {
            return Err(
                Error::new(codes::FONT_OUTLINE_INVALID).with_arg("family", font.family.clone())
            );
        }
        faces.insert(blob.id(), font.clone());
    }
    let mut font_cx = FontContext {
        collection,
        source_cache: SourceCache::default(),
    };
    let mut layout_cx: LayoutContext<usize> = LayoutContext::new();
    let mut count = 0_usize;
    let mut top = 0.0_f64;
    let mut first_baseline = None;
    let mut widest = 0.0_f64;
    for (pi, (paragraph, families)) in input.paragraphs.iter().zip(&families).enumerate() {
        top -= paragraph.space_before;
        let mut text = String::new();
        let mut ranges = Vec::new();
        for run in &paragraph.runs {
            let start = text.len();
            text.push_str(&run.text);
            if run.stack != Stack::None {
                text.push('/');
                text.push_str(run.denominator.as_deref().unwrap_or(""));
            }
            ranges.push(start..text.len());
        }
        let mut builder = layout_cx.ranged_builder(&mut font_cx, &text, 1.0, false);
        builder.push_default(StyleProperty::FontSize(f32_of(input.height)));
        builder.push_default(StyleProperty::LineHeight(
            parley::style::LineHeight::FontSizeRelative(f32_of(1.2 * paragraph.line_spacing)),
        ));
        for (ri, (run, range)) in paragraph.runs.iter().zip(&ranges).enumerate() {
            let family = families.get(ri).cloned().unwrap_or_default();
            builder.push(
                StyleProperty::FontFamily(FontFamily::Single(FontFamilyName::Named(Cow::Owned(
                    family,
                )))),
                range.clone(),
            );
            builder.push(StyleProperty::Brush(ri), range.clone());
            if let Some(h) = run.height {
                builder.push(StyleProperty::FontSize(f32_of(h)), range.clone());
            }
            if run.bold {
                builder.push(StyleProperty::FontWeight(FontWeight::BOLD), range.clone());
            }
            if run.italic {
                builder.push(StyleProperty::FontStyle(FontStyle::Italic), range.clone());
            }
        }
        let mut layout = builder.build(&text);
        let width = (input.width > 0.0).then(|| f32_of(input.width));
        layout.break_all_lines(width);
        let align = match paragraph.alignment {
            Alignment::Left => Align::Left,
            Alignment::Center => Align::Center,
            Alignment::Right => Align::Right,
            Alignment::Justify => Align::Justify,
        };
        layout.align(align, AlignmentOptions::default());
        for line in layout.lines() {
            let m = line.metrics();
            let baseline = top - f64::from(m.baseline);
            first_baseline.get_or_insert(baseline);
            let left = f64::from(m.inline_min_coord);
            let right = f64::from(m.inline_max_coord);
            widest = widest.max(right);
            out.lines.push(LineBox {
                paragraph: pi,
                column: 0,
                left,
                right,
                baseline,
                ascent: f64::from(m.baseline - m.block_min_coord),
                descent: f64::from(m.block_max_coord - m.baseline),
            });
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run_index = glyph_run.style().brush;
                let font = glyph_run.run().font();
                let face = faces.get(&font.data.id()).cloned().unwrap_or_else(|| {
                    crate::fonts::OutlineFont {
                        index: font.index,
                        ..base_font.clone()
                    }
                });
                let glyphs: Vec<PositionedGlyph> = glyph_run
                    .positioned_glyphs()
                    .map(|g| PositionedGlyph {
                        id: g.id,
                        position: Point2::new(f64::from(g.x), top - f64::from(g.y)),
                    })
                    .collect();
                count = count.saturating_add(glyphs.len());
                if count > options.max_items {
                    return Err(too_large(options.max_items));
                }
                let x0 = f64::from(glyph_run.offset());
                let x1 = x0 + f64::from(glyph_run.advance());
                let size = f64::from(glyph_run.run().font_size());
                let run = paragraph.runs.get(run_index);
                let color = run
                    .and_then(|r| r.color)
                    .map(|[r, g, b, a]| Color::new(r, g, b, a));
                let mut curves = Vec::new();
                for (on, at) in [
                    (run.is_some_and(|r| r.underline), -0.1),
                    (run.is_some_and(|r| r.overline), 0.85),
                    (run.is_some_and(|r| r.strike), 0.3),
                ] {
                    if on && x1 > x0 {
                        let y = baseline + at * size;
                        curves.push(Curve2::Segment(Segment2::new(
                            Point2::new(x0, y),
                            Point2::new(x1, y),
                        )?));
                    }
                }
                out.runs.push(GlyphRun {
                    paragraph: pi,
                    run: run_index,
                    color,
                    kind: RunKind::Outline {
                        font: face,
                        size,
                        glyphs,
                    },
                });
                if !curves.is_empty() {
                    out.runs.push(GlyphRun {
                        paragraph: pi,
                        run: run_index,
                        color,
                        kind: RunKind::Stroke(curves),
                    });
                }
            }
        }
        top -= f64::from(layout.height()) + paragraph.space_after;
    }
    let region = if input.width > 0.0 {
        input.width
    } else {
        widest
    };
    let bottom = out
        .lines
        .iter()
        .map(|l| l.baseline - l.descent)
        .fold(0.0, f64::min);
    let ax = match input.halign {
        HAlign::Left => 0.0,
        HAlign::Center => region * 0.5,
        HAlign::Right => region,
    };
    let ay = match input.valign {
        VAlign::Baseline => first_baseline.unwrap_or(0.0),
        VAlign::Bottom => bottom,
        VAlign::Middle => bottom * 0.5,
        VAlign::Top => 0.0,
    };
    shift(out, ax, ay, &options.tolerance)?;
    out.bounds = bounds(&out.lines);
    Ok(())
}

/// Moves everything by `(-ax, -ay)`.
fn shift(out: &mut TextLayout, ax: f64, ay: f64, tol: &arc_base::Tolerance) -> Result<()> {
    if ax == 0.0 && ay == 0.0 {
        return Ok(());
    }
    for line in &mut out.lines {
        line.left -= ax;
        line.right -= ax;
        line.baseline -= ay;
    }
    let t = arc_base::math::Transform2::from_matrix_unchecked(arc_base::math::Matrix3::new(
        1.0, 0.0, -ax, 0.0, 1.0, -ay, 0.0, 0.0, 1.0,
    ));
    for run in &mut out.runs {
        match &mut run.kind {
            RunKind::Outline { glyphs, .. } => {
                for g in glyphs {
                    g.position = Point2::new(g.position.x - ax, g.position.y - ay);
                }
            }
            RunKind::Stroke(curves) => {
                for c in curves.iter_mut() {
                    *c = c.transformed(&t, tol)?;
                }
            }
        }
    }
    Ok(())
}
