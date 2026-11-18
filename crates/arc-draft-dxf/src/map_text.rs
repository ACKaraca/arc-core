//! Mapping of text, multi-line text and dimensions.

use arc_base::math::Point2;
use arc_draft::dimension::Kind as DimensionKind;
use arc_draft::text::paragraphs_from_plain;
use arc_draft::types::TEXT_BYTES;
use arc_io_dxf::model::{Dimension, MText, Text};
use arc_io_dxf::{Entity, EntityKind};

use crate::error::reasons;
use crate::geo::{pt, rad};
use crate::map_tables::{clean_line, truncate};
use crate::mapper::{Built, Mapper};
use crate::plan::{DimensionSpec, RichTextSpec, Shape, TextSpec};

/// Replaces the percent codes of single-line text; `true` when a code had no replacement and
/// was dropped (underline, overline and strike-through switches).
pub(crate) fn percent_codes(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut dropped = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' || chars.peek() != Some(&'%') {
            out.push(c);
            continue;
        }
        chars.next();
        match chars.peek().copied() {
            Some('d' | 'D') => out.push('\u{00B0}'),
            Some('p' | 'P') => out.push('\u{00B1}'),
            Some('c' | 'C') => out.push('\u{2300}'),
            Some('%') => out.push('%'),
            Some('u' | 'U' | 'o' | 'O' | 'k' | 'K') => dropped = true,
            Some(d) if d.is_ascii_digit() => {
                let digits: String = chars
                    .clone()
                    .take(3)
                    .take_while(char::is_ascii_digit)
                    .collect();
                for _ in 0..digits.len().saturating_sub(1) {
                    chars.next();
                }
                match digits.parse::<u32>().ok().and_then(char::from_u32) {
                    Some(ch) => out.push(ch),
                    None => dropped = true,
                }
            }
            _ => {
                out.push('%');
                out.push('%');
                continue;
            }
        }
        chars.next();
    }
    (out, dropped)
}

/// Multi-line text content without formatting codes: paragraph breaks become line feeds,
/// stacked fractions keep their slash. The flag says whether any code or grouping brace was
/// removed.
pub(crate) fn strip_mtext(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut formatted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '{' | '}' => formatted = true,
            '\\' => match chars.next() {
                Some('P' | 'N') => out.push('\n'),
                Some('~') => out.push(' '),
                Some(c @ ('\\' | '{' | '}')) => out.push(c),
                Some('L' | 'l' | 'O' | 'o' | 'K' | 'k') => formatted = true,
                Some('S' | 's') => {
                    formatted = true;
                    for c in chars.by_ref().take_while(|c| *c != ';') {
                        out.push(if c == '^' { '/' } else { c });
                    }
                }
                Some(_) => {
                    formatted = true;
                    for c in chars.by_ref() {
                        if c == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            c => out.push(c),
        }
    }
    let (out, dropped) = percent_codes(&out);
    (out, formatted || dropped)
}

impl Mapper<'_> {
    pub(crate) fn text_shape(&mut self, e: &Entity, f: f64) -> Option<Built> {
        match &e.kind {
            EntityKind::Text(t) => self.text(e, t, f),
            EntityKind::MText(m) => self.mtext(e, m, f),
            _ => None,
        }
    }

    /// Text content cleaned for the drafting model; noted when anything changed.
    fn content(&mut self, e: &Entity, raw: &str, formatted: bool, single_line: bool) -> String {
        let bound = if single_line {
            TEXT_BYTES
        } else {
            self.o.limits.string_bytes
        };
        let max = usize::try_from(bound).unwrap_or(usize::MAX);
        let controls: String = if single_line {
            clean_line(raw)
        } else {
            raw.chars()
                .filter(|c| !c.is_control() || matches!(c, '\t' | '\n'))
                .collect()
        };
        let cut = truncate(&controls, max);
        if cut != raw || formatted {
            if formatted && matches!(e.kind, EntityKind::MText(_)) {
                self.note(reasons::TEXT_FORMATTING, "MTEXT", e.source_id());
            } else {
                self.note(reasons::TEXT_ADJUSTED, e.kind.type_name(), e.source_id());
            }
        }
        cut
    }

    fn text(&mut self, e: &Entity, t: &Text, f: f64) -> Option<Built> {
        let (decoded, dropped) = percent_codes(&t.value.replace(['\r', '\n'], " "));
        let content = self.content(e, &decoded, dropped, true);
        let moved = t.horizontal.unwrap_or(0) != 0 || t.vertical.unwrap_or(0) != 0;
        let at = match t.align {
            Some(a) if moved => a,
            _ => t.insert,
        };
        let (halign, valign) = match (t.horizontal.unwrap_or(0), t.vertical.unwrap_or(0)) {
            (4, _) => (1, 2),
            (h, v) => (
                match h {
                    1 => 1,
                    2 => 2,
                    _ => 0,
                },
                u32::try_from(v).unwrap_or(0).min(3),
            ),
        };
        if matches!(t.horizontal, Some(3 | 5)) {
            self.note_what(reasons::VALUE_ADJUSTED, e, "text_alignment");
        }
        if t.width_factor
            .is_some_and(|w| (w - 1.0).abs() > f64::EPSILON)
            || t.oblique.is_some_and(|o| o != 0.0)
        {
            self.note_what(reasons::VALUE_ADJUSTED, e, "text_width_or_oblique");
        }
        let style = t
            .style
            .as_deref()
            .map(|s| self.text_style_name(s, e.source_id()));
        let spec = TextSpec {
            content,
            position: pt([at[0], at[1]], f),
            rotation: rad(t.rotation.unwrap_or(0.0)),
            style,
            height: t.height.max(0.0) * f,
            halign,
            valign,
        };
        Built::with_z(Shape::Text(spec), vec![t.insert[2]])
    }

    fn mtext(&mut self, e: &Entity, m: &MText, f: f64) -> Option<Built> {
        let (stripped, formatted) = strip_mtext(&m.text);
        let content = self.content(e, &stripped, formatted, false);
        let paragraphs = match paragraphs_from_plain(&content, &self.o.limits) {
            Ok(p) => p,
            Err(_) => {
                self.note(reasons::GEOMETRY_INVALID, "MTEXT", e.source_id());
                return None;
            }
        };
        let rotation = match (m.rotation, m.x_axis) {
            (Some(r), _) => rad(r),
            (None, Some(x)) if x[0] != 0.0 || x[1] != 0.0 => x[1].atan2(x[0]),
            _ => 0.0,
        };
        let style = m
            .style
            .as_deref()
            .map(|s| self.text_style_name(s, e.source_id()));
        let spec = RichTextSpec {
            paragraphs,
            position: pt([m.insert[0], m.insert[1]], f),
            rotation,
            style,
            height: m.height.max(0.0) * f,
            width: m.width.unwrap_or(0.0).max(0.0) * f,
            attachment: u32::try_from(m.attachment.unwrap_or(1) - 1)
                .unwrap_or(0)
                .min(8),
        };
        Built::with_z(Shape::RichText(spec), vec![m.insert[2]])
    }

    pub(crate) fn dimension(&mut self, e: &Entity, d: &Dimension, f: f64) -> Option<Built> {
        if d.kind & 7 == 7 {
            self.note(reasons::ENTITY_UNMAPPED, "DIMENSION", e.source_id());
            return None;
        }
        let p2 = |p: [f64; 3]| pt([p[0], p[1]], f);
        let Some((kind, points, line_position)) = dimension_parts(d, f) else {
            self.note(reasons::GEOMETRY_INVALID, "DIMENSION", e.source_id());
            return None;
        };
        let text_override = d
            .text
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| self.content(e, &t.replace("<>", "{}"), false, true));
        let style = d
            .style
            .as_deref()
            .map(|s| self.dim_style_name(s, e.source_id()));
        let spec = DimensionSpec {
            kind,
            points,
            line_position,
            rotation: rad(d.angle.unwrap_or(0.0)),
            text_override,
            text_position: (d.kind & 128 != 0).then(|| p2(d.text_mid)),
            style,
        };
        self.note(
            reasons::DIMENSION_NONASSOCIATIVE,
            "DIMENSION",
            e.source_id(),
        );
        Built::with_z(Shape::Dimension(spec), vec![d.def_point[2]])
    }
}

/// The kind, definition points and dimension line point of a dimension, `None` when a point
/// its kind needs is missing.
fn dimension_parts(d: &Dimension, f: f64) -> Option<(DimensionKind, Vec<Point2>, Point2)> {
    let p2 = |p: [f64; 3]| pt([p[0], p[1]], f);
    let opt = |p: Option<[f64; 3]>| p.map(p2);
    Some(match d.kind & 7 {
        0 | 1 => (
            if d.kind & 7 == 0 {
                DimensionKind::Linear
            } else {
                DimensionKind::Aligned
            },
            vec![opt(d.point13)?, opt(d.point14)?],
            p2(d.def_point),
        ),
        2 => (
            DimensionKind::AngularLines,
            vec![
                opt(d.point13)?,
                opt(d.point14)?,
                opt(d.point15)?,
                p2(d.def_point),
            ],
            opt(d.point16)?,
        ),
        3 => {
            let on_curve = opt(d.point15)?;
            let other = p2(d.def_point);
            let center = Point2::new((on_curve.x + other.x) / 2.0, (on_curve.y + other.y) / 2.0);
            (
                DimensionKind::Diameter,
                vec![center, on_curve],
                p2(d.text_mid),
            )
        }
        4 => (
            DimensionKind::Radial,
            vec![p2(d.def_point), opt(d.point15)?],
            p2(d.text_mid),
        ),
        5 => (
            DimensionKind::AngularPoints,
            vec![opt(d.point15)?, opt(d.point13)?, opt(d.point14)?],
            p2(d.def_point),
        ),
        _ => (
            if d.kind & 64 != 0 {
                DimensionKind::OrdinateX
            } else {
                DimensionKind::OrdinateY
            },
            vec![p2(d.def_point), opt(d.point13)?],
            opt(d.point14).unwrap_or_else(|| p2(d.text_mid)),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_codes_become_characters() {
        assert_eq!(
            percent_codes("45%%d %%p0.1 %%c20 %%065"),
            ("45\u{b0} \u{b1}0.1 \u{2300}20 A".to_owned(), false)
        );
        assert_eq!(percent_codes("%%uunder%%o"), ("under".to_owned(), true));
        assert_eq!(percent_codes("100%"), ("100%".to_owned(), false));
        assert_eq!(percent_codes("a%%zb"), ("a%%zb".to_owned(), false));
    }

    #[test]
    fn multi_line_formatting_codes_are_stripped() {
        assert_eq!(strip_mtext("one\\Ptwo"), ("one\ntwo".to_owned(), false));
        assert_eq!(
            strip_mtext("{\\fArial|b1;Bold} text"),
            ("Bold text".to_owned(), true)
        );
        assert_eq!(strip_mtext("\\S1/2;\\~x"), ("1/2 x".to_owned(), true));
        assert_eq!(strip_mtext("a\\\\b\\{c\\}"), ("a\\b{c}".to_owned(), false));
        assert_eq!(
            strip_mtext("\\Lunder\\l \\H2x;big"),
            ("under big".to_owned(), true)
        );
        assert_eq!(strip_mtext("trailing\\"), ("trailing".to_owned(), false));
    }
}
