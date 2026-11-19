//! Outline text through `parley` (feature `outline-text`): shaping with registered bytes only,
//! bidirectional reordering, wrapping, determinism and the glyph primitive of display lists.
//! The test font is built here: a minimal TrueType file with square glyphs.

#![cfg(feature = "outline-text")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use std::sync::Arc;

use arc_base::Tolerance;
use arc_draft::display::{DisplayOptions, Lists, Shape, space_list};
use arc_draft::fonts::{FontRegistry, Fonts, SubstitutedBy, Substitutions};
use arc_draft::text::{Paragraph, Run};
use arc_draft::text_layout::{
    Columns, HAlign, LayoutOptions, RunKind, TextInput, TextLayout, TextStyle, VAlign, layout,
};
use serde_json::json;
use support::Fixture;

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Glyph 0 is empty, glyphs 1..=5 are squares for `a`..`e`, 6 is the space, 7 and 8 are
/// squares for U+05D0 and U+05D1. Every advance is 600 of 1000 units.
fn test_font() -> Vec<u8> {
    let glyphs = 9_u16;
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in 0..glyphs {
        be16(&mut loca, (glyf.len() / 2) as u16);
        if g == 0 || g == 6 {
            continue;
        }
        let side = 100 + 40 * g as i16;
        be16(&mut glyf, 1); // contours
        for v in [50, 0, 50 + side, side] {
            be16(&mut glyf, v as u16);
        }
        be16(&mut glyf, 3); // end point
        be16(&mut glyf, 0); // instructions
        glyf.extend_from_slice(&[1, 1, 1, 1]);
        for dx in [50, side, 0, -side] {
            be16(&mut glyf, dx as u16);
        }
        for dy in [0, 0, side, 0] {
            be16(&mut glyf, dy as u16);
        }
    }
    be16(&mut loca, (glyf.len() / 2) as u16);
    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0);
    be32(&mut head, 0x5F0F_3CF5);
    be16(&mut head, 0x000B);
    be16(&mut head, 1000);
    head.extend_from_slice(&[0; 16]);
    for v in [0_u16, 0, 600, 700, 0, 8, 2, 0, 0] {
        be16(&mut head, v);
    }
    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for v in [800_i16, -200, 0, 600, 0, 0, 600, 1, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut hhea, v as u16);
    }
    be16(&mut hhea, glyphs);
    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, glyphs);
    for v in [4_u16, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut maxp, v);
    }
    let mut hmtx = Vec::new();
    for _ in 0..glyphs {
        be16(&mut hmtx, 600);
        be16(&mut hmtx, 50);
    }
    // cmap format 4: segments (start, end, delta).
    let segments: [(u16, u16, u16); 4] = [
        (0x20, 0x20, 6_u16.wrapping_sub(0x20)),
        (0x61, 0x65, 1_u16.wrapping_sub(0x61)),
        (0x5D0, 0x5D1, 7_u16.wrapping_sub(0x5D0)),
        (0xFFFF, 0xFFFF, 1),
    ];
    let mut sub = Vec::new();
    let n = segments.len() as u16;
    be16(&mut sub, 4);
    be16(&mut sub, 16 + 8 * n);
    be16(&mut sub, 0);
    be16(&mut sub, 2 * n);
    be16(&mut sub, 8);
    be16(&mut sub, 2);
    be16(&mut sub, 0);
    segments.iter().for_each(|s| be16(&mut sub, s.1));
    be16(&mut sub, 0);
    segments.iter().for_each(|s| be16(&mut sub, s.0));
    segments.iter().for_each(|s| be16(&mut sub, s.2));
    segments.iter().for_each(|_| be16(&mut sub, 0));
    let mut cmap = Vec::new();
    for v in [0_u16, 1, 3, 1] {
        be16(&mut cmap, v);
    }
    be32(&mut cmap, 12);
    cmap.extend(sub);
    let family: Vec<u8> = "Test".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    for v in [0_u16, 1, 18, 3, 1, 0x409, 1, family.len() as u16, 0] {
        be16(&mut name, v);
    }
    name.extend(family);
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"name", name),
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    tables.sort_by_key(|t| *t.0);
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, tables.len() as u16);
    for v in [64_u16, 2, 48] {
        be16(&mut out, v);
    }
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0);
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out
}

fn fonts() -> Fonts {
    let mut registry = FontRegistry::with_builtin().unwrap();
    registry.add_outline("Test Sans", Arc::from(test_font()), 0);
    Fonts {
        provider: Arc::new(registry),
        ..Fonts::default()
    }
}

fn lay(text: &[Run], width: f64, fonts: &Fonts) -> TextLayout {
    let p = vec![Paragraph {
        runs: text.to_vec(),
        ..Paragraph::default()
    }];
    let style = TextStyle {
        font: Some("test sans".into()),
        ..TextStyle::default()
    };
    let input = TextInput {
        paragraphs: &p,
        height: 10.0,
        width,
        halign: HAlign::Left,
        valign: VAlign::Top,
        columns: Columns::default(),
        style: &style,
    };
    let options = LayoutOptions {
        tolerance: Tolerance::DEFAULT,
        max_items: 10_000,
    };
    layout(&input, &Substitutions::default(), fonts, &options).unwrap()
}

fn run(text: &str) -> Run {
    Run {
        text: text.into(),
        ..Run::default()
    }
}

fn glyph_ids(out: &TextLayout) -> Vec<Vec<u32>> {
    out.runs
        .iter()
        .filter_map(|r| match &r.kind {
            RunKind::Outline { glyphs, .. } => Some(glyphs.iter().map(|g| g.id).collect()),
            RunKind::Stroke(_) => None,
        })
        .collect()
}

#[test]
fn outline_text_shapes_reorders_wraps_and_is_deterministic() {
    let fonts = fonts();
    let out = lay(&[run("abc")], 0.0, &fonts);
    assert_eq!(glyph_ids(&out), vec![vec![1, 2, 3]]);
    let RunKind::Outline { font, size, glyphs } = &out.runs[0].kind else {
        panic!()
    };
    assert_eq!(font.family, "Test Sans");
    assert!((size - 10.0).abs() < 1e-6);
    assert!((glyphs[1].position.x - glyphs[0].position.x - 6.0).abs() < 1e-4);
    // Right-to-left characters are reordered: the visual order reverses the logical one.
    let rtl = lay(&[run("\u{5D0}\u{5D1}")], 0.0, &fonts);
    assert_eq!(glyph_ids(&rtl), vec![vec![8, 7]]);
    // Wrapping at the width puts each word on its own line.
    let wrapped = lay(&[run("abcde abcde abcde")], 40.0, &fonts);
    assert_eq!(wrapped.lines.len(), 3);
    assert_eq!(wrapped, lay(&[run("abcde abcde abcde")], 40.0, &fonts));
    // A run naming the stroke font uses the base outline font.
    let mixed = lay(
        &[Run {
            text: "a".into(),
            font: Some("Arc Stroke".into()),
            underline: true,
            ..Run::default()
        }],
        0.0,
        &fonts,
    );
    assert_eq!(mixed.substitutions[0].by, SubstitutedBy::Base);
    assert!(
        mixed
            .runs
            .iter()
            .any(|r| matches!(r.kind, RunKind::Stroke(_)))
    );
}

#[test]
fn unreadable_outline_bytes_fall_back_to_the_bundled_font() {
    let mut registry = FontRegistry::with_builtin().unwrap();
    registry.add_outline("Broken", Arc::from(vec![0_u8, 1, 0, 0, 9, 9]), 0);
    let fonts = Fonts {
        provider: Arc::new(registry),
        ..Fonts::default()
    };
    let p = vec![Paragraph {
        runs: vec![run("a")],
        ..Paragraph::default()
    }];
    let style = TextStyle {
        font: Some("Broken".into()),
        ..TextStyle::default()
    };
    let input = TextInput {
        paragraphs: &p,
        height: 1.0,
        width: 0.0,
        halign: HAlign::Left,
        valign: VAlign::Top,
        columns: Columns::default(),
        style: &style,
    };
    let options = LayoutOptions {
        tolerance: Tolerance::DEFAULT,
        max_items: 10,
    };
    let out = layout(&input, &Substitutions::default(), &fonts, &options).unwrap();
    let bundled = arc_draft::fonts::BUNDLED_OUTLINE_FAMILY;
    assert_eq!(
        out.substitutions,
        vec![arc_draft::fonts::Substitution {
            requested: "Broken".into(),
            used: bundled.into(),
            by: arc_draft::fonts::SubstitutedBy::Bundled,
        }]
    );
    assert!(matches!(
        &out.runs[0].kind,
        RunKind::Outline { font, .. } if font.family == bundled
    ));
}

#[test]
fn outline_text_entities_contribute_glyph_primitives() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "sans"}),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "font",
        json!("Test Sans"),
    );
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "abc", "position": [100.0, 0.0],
               "rotation": 0.5, "style": style.to_string(), "height": 10.0}),
    );
    let options = DisplayOptions {
        fonts: fonts(),
        ..DisplayOptions::default()
    };
    let snapshot = f.document.snapshot();
    let list = space_list(&snapshot, Lists::Direct, space, &options).unwrap();
    let glyphs: Vec<_> = list
        .primitives
        .iter()
        .filter(|p| p.source == text)
        .collect();
    assert_eq!(glyphs.len(), 1);
    let Shape::Glyphs {
        font,
        glyphs: g,
        placement,
        ..
    } = &glyphs[0].shape
    else {
        panic!("{:?}", glyphs[0].shape)
    };
    assert_eq!(font.family, "Test Sans");
    assert_eq!(g.len(), 3);
    let origin = placement * arc_base::math::Point2::origin();
    assert!((origin.x - 100.0).abs() < 1e-9);
}
