//! Text and fonts (CORE-DRAFT-006, 034, 035, 036): commands and package round trip, layout
//! determinism and the golden paragraph, line breaking and justification properties, the
//! stroke-font reader, the shipped font's coverage, substitution against a reference model, and
//! display lists with their regeneration.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arc_base::{Limits, ObjectId, Tolerance};
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::display::{self, DisplayOptions, Lists, Shape, space_list};
use arc_draft::fonts::{
    FontRegistry, Fonts, SubstitutedBy, Substitution, Substitutions, document_table, resolve,
};
use arc_draft::stroke_font::{BUILTIN_NAME, StrokeFont, builtin};
use arc_draft::text::{Alignment, Paragraph, Run, Stack};
use arc_draft::text_layout::{
    Columns, HAlign, LayoutOptions, TextInput, TextLayout, TextStyle, VAlign, layout,
};
use arc_draft::types;
use proptest::prelude::*;
use serde_json::json;
use support::Fixture;

const TOL: f64 = 1e-9;

fn options() -> LayoutOptions {
    LayoutOptions {
        tolerance: Tolerance::DEFAULT,
        max_items: 1_000_000,
    }
}

fn run(text: &str) -> Run {
    Run {
        text: text.into(),
        ..Run::default()
    }
}

fn lay(paragraphs: &[Paragraph], height: f64, width: f64) -> TextLayout {
    let style = TextStyle::default();
    let input = TextInput {
        paragraphs,
        height,
        width,
        halign: HAlign::Left,
        valign: VAlign::Top,
        columns: Columns::default(),
        style: &style,
    };
    layout(
        &input,
        &Substitutions::default(),
        &Fonts::default(),
        &options(),
    )
    .unwrap()
}

/// 64-bit FNV-1a of the exact debug text (floats print round-trip).
fn digest(layout: &TextLayout) -> String {
    let text = format!("{layout:?}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn golden_paragraph() -> Vec<Paragraph> {
    vec![Paragraph {
        alignment: Alignment::Justify,
        tabs: vec![20.0],
        runs: vec![
            run("Kat\tÇizim ölçeği ŞĞİÜ çığır ışık iğne öğrenci düğüm şapka"),
            Run {
                text: "3".into(),
                stack: Stack::Horizontal,
                denominator: Some("4".into()),
                ..Run::default()
            },
            Run {
                text: " Ağaç".into(),
                underline: true,
                ..Run::default()
            },
        ],
        ..Paragraph::default()
    }]
}

#[test]
fn the_golden_paragraph_is_byte_identical_and_justified() {
    let paragraphs = golden_paragraph();
    let a = lay(&paragraphs, 2.5, 60.0);
    let b = lay(&paragraphs, 2.5, 60.0);
    assert_eq!(a, b);
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    assert!(a.lines.len() >= 3, "{:?}", a.lines);
    assert_eq!(a.missing, 0);
    assert!(a.substitutions.is_empty());
    for line in &a.lines[..a.lines.len() - 1] {
        assert!((line.right - 60.0).abs() < TOL, "{line:?}");
    }
    let summary: String = a
        .lines
        .iter()
        .map(|l| {
            format!(
                "{} {} {:?} {:?} {:?} {:?} {:?}\n",
                l.paragraph, l.column, l.left, l.right, l.baseline, l.ascent, l.descent
            )
        })
        .collect::<String>()
        + &format!("runs {} digest {}\n", a.runs.len(), digest(&a));
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/text_paragraph.txt"
    );
    if std::env::var_os("ARC_BLESS").is_some() {
        std::fs::write(path, &summary).unwrap();
    }
    assert_eq!(summary, std::fs::read_to_string(path).unwrap());
}

#[test]
fn tabs_fractions_decorations_and_anchors_follow_the_rules() {
    let font = builtin().unwrap();
    let h = 2.0;
    let s = h / font.cap_height;
    // A tab moves to the stop; past the stops, to the next multiple of four heights.
    let p = vec![Paragraph {
        tabs: vec![10.0],
        runs: vec![run("A\tB\tC")],
        ..Paragraph::default()
    }];
    let out = lay(&p, h, 0.0);
    let line = out.lines[0];
    let advance = |c: char| font.glyph(c).unwrap().advance * s;
    assert!((line.right - (16.0 + advance('C'))).abs() < TOL, "{line:?}");
    // The first baseline is one height below the top; Top anchoring puts the top at 0.
    assert!((line.baseline + h).abs() < TOL);
    // Baseline anchoring puts the first baseline at 0.
    let style = TextStyle::default();
    let input = TextInput {
        paragraphs: &p,
        height: h,
        width: 0.0,
        halign: HAlign::Right,
        valign: VAlign::Baseline,
        columns: Columns::default(),
        style: &style,
    };
    let anchored = layout(
        &input,
        &Substitutions::default(),
        &Fonts::default(),
        &options(),
    )
    .unwrap();
    assert!(anchored.lines[0].baseline.abs() < TOL);
    assert!(anchored.lines[0].right.abs() < TOL);
    // A horizontal fraction is as wide as its wider part plus 0.2 heights, with one bar.
    let frac = vec![Paragraph {
        runs: vec![Run {
            text: "1".into(),
            stack: Stack::Horizontal,
            denominator: Some("16".into()),
            underline: true,
            ..Run::default()
        }],
        ..Paragraph::default()
    }];
    let out = lay(&frac, h, 0.0);
    let expected = (advance('1') + advance('6')) * 0.6 + 0.2 * h;
    assert!((out.lines[0].right - expected).abs() < TOL);
    // Missing characters draw the missing glyph and are counted.
    let out = lay(
        &[Paragraph {
            runs: vec![run("a\u{4e00}b")],
            ..Paragraph::default()
        }],
        h,
        0.0,
    );
    assert_eq!(out.missing, 1);
}

#[test]
fn columns_flow_and_paragraph_spacing_applies() {
    let p: Vec<Paragraph> = (0..6)
        .map(|i| Paragraph {
            space_after: 1.0,
            runs: vec![run(&format!("line {i}"))],
            ..Paragraph::default()
        })
        .collect();
    let style = TextStyle::default();
    let input = TextInput {
        paragraphs: &p,
        height: 2.0,
        width: 30.0,
        halign: HAlign::Left,
        valign: VAlign::Top,
        columns: Columns {
            count: 2,
            gutter: 5.0,
            height: 14.0,
        },
        style: &style,
    };
    let out = layout(
        &input,
        &Substitutions::default(),
        &Fonts::default(),
        &options(),
    )
    .unwrap();
    let columns: BTreeSet<u32> = out.lines.iter().map(|l| l.column).collect();
    assert_eq!(columns, BTreeSet::from([0, 1]));
    for l in out.lines.iter().filter(|l| l.column == 1) {
        assert!(l.left >= 35.0 - TOL);
    }
    for l in out.lines.iter().filter(|l| l.column == 0) {
        assert!(l.baseline - l.descent >= -14.0 - TOL);
    }
}

fn words_strategy() -> impl Strategy<Value = Vec<String>> {
    let word = proptest::collection::vec(
        prop::sample::select(vec!['a', 'm', 'W', 'ı', 'Ş', 'ğ', '1', 'Ж', 'λ']),
        1..9,
    )
    .prop_map(|c| c.into_iter().collect::<String>());
    proptest::collection::vec(word, 1..30)
}

fn width_of(font: &StrokeFont, word: &str, s: f64) -> f64 {
    let chars: Vec<char> = word.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let kern = chars.get(i + 1).map_or(0.0, |n| font.kern(*c, *n));
            (font.glyph(*c).unwrap().advance + kern) * s
        })
        .sum()
}

/// The breaking and justification rules on one input: no line exceeds the width unless one word
/// does, and a justified line that is not the last and holds more than one word ends at the margin.
fn check_breaking(words: &[String], height: f64, width: f64) -> Result<(), TestCaseError> {
    let font = builtin().unwrap();
    let p = vec![Paragraph {
        alignment: Alignment::Justify,
        runs: vec![run(&words.join(" "))],
        ..Paragraph::default()
    }];
    let out = lay(&p, height, width);
    let s = height / font.cap_height;
    let widths: Vec<f64> = words.iter().map(|w| width_of(&font, w, s)).collect();
    let tol = 1e-9 * width.max(1.0);
    let last = out.lines.len() - 1;
    for (i, line) in out.lines.iter().enumerate() {
        let extent = line.right - line.left;
        if line.right > width + tol {
            prop_assert!(widths.iter().any(|w| (w - extent).abs() <= tol), "{line:?}");
        }
        let one_word = widths.iter().any(|w| (w - extent).abs() <= tol);
        if i < last && extent < width && !one_word {
            prop_assert!((line.right - width).abs() <= tol, "{line:?} {width}");
        }
    }
    prop_assert_eq!(out.clone(), lay(&p, height, width));
    Ok(())
}

/// Issue 76: at a text height of 0.001 the absolute linear tolerance (1e-6) was a tenth of a
/// percent of the height, so a line took a word that ended up to 1e-6 beyond the width.
#[test]
fn regression_76_small_text_does_not_overflow_by_the_absolute_tolerance() {
    let font = builtin().unwrap();
    let height = 1e-3;
    let s = height / font.cap_height;
    let words = vec!["ğŞı".to_string(), "Ж".to_string()];
    let w: Vec<f64> = words.iter().map(|x| width_of(&font, x, s)).collect();
    let space = font.glyph(' ').unwrap().advance * s;
    for base in [w[0], w[1], w[0] + w[1] + space] {
        for d in [-2e-6, -1e-6, -1e-7, -1e-9, 0.0, 1e-9, 1e-7, 1e-6, 2e-6] {
            if base + d >= 1.0 * height {
                check_breaking(&words, height, base + d).unwrap();
            }
        }
    }
    // The width that failed: just below the two words and the space.
    let below = w[0] + w[1] + space - 1e-6;
    let out = lay(
        &[Paragraph {
            runs: vec![run("ğŞı Ж")],
            ..Paragraph::default()
        }],
        height,
        below,
    );
    assert_eq!(out.lines.len(), 2);
}

proptest! {
    #[test]
    fn no_line_exceeds_the_width_unless_one_word_does_and_justified_lines_end_at_the_margin(
        words in words_strategy(),
        width in 1.0f64..200.0,
        height in prop_oneof![Just(1e-3), 0.5f64..5.0, Just(1e3)],
    ) {
        check_breaking(&words, height, width * height)?;
    }

    #[test]
    fn hostile_font_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = StrokeFont::parse(&bytes, &Limits::DEFAULT);
    }

    #[test]
    fn mutated_fonts_never_panic(position in 0usize..4000, byte in any::<u8>(), cut in 0usize..4000) {
        let mut bytes = SMALL_FONT.as_bytes().to_vec();
        let n = bytes.len();
        bytes[position % n] = byte;
        bytes.truncate(cut % (n + 1));
        let _ = StrokeFont::parse(&bytes, &Limits::DEFAULT);
    }
}

const SMALL_FONT: &str = "arc-stroke-font 1
name Test
units 10
cap-height 5
x-height 3
ascender 8
descender -2
line-spacing 9
missing 3f
# a comment
glyph 41 6
l 0 0 3 5 6 0
a 3 2 1 0 360
end
glyph 3f 4
l 1 4 3 4 3 2
end
kern 41 3f -0.5
";

#[test]
fn golden_vectors_of_the_stroke_font_format() {
    let font = StrokeFont::parse(SMALL_FONT.as_bytes(), &Limits::DEFAULT).unwrap();
    assert_eq!(font.name, "Test");
    assert_eq!(font.glyphs.len(), 2);
    assert_eq!(font.kern('A', '?'), -0.5);
    assert_eq!(font.missing, Some('?'));
    let text = font.to_text();
    assert_eq!(
        text,
        "arc-stroke-font 1\nname Test\nunits 10\ncap-height 5\nx-height 3\nascender 8\n\
         descender -2\nline-spacing 9\nmissing 3f\nglyph 3f 4\nl 1 4 3 4 3 2\nend\n\
         glyph 41 6\nl 0 0 3 5 6 0\na 3 2 1 0 360\nend\nkern 41 3f -0.5\n"
    );
    assert_eq!(
        StrokeFont::parse(text.as_bytes(), &Limits::DEFAULT).unwrap(),
        font
    );
    let code = |text: &str| {
        StrokeFont::parse(text.as_bytes(), &Limits::DEFAULT)
            .unwrap_err()
            .code()
    };
    let cases: [(&str, &str); 12] = [
        ("", "draft.font.syntax"),
        ("arc-stroke-font 7\n", "draft.font.version_unsupported"),
        ("arc-stroke-font 1\n", "draft.font.header_invalid"),
        (
            &SMALL_FONT.replace("glyph 3f 4", "glyph 41 4"),
            "draft.font.glyph_duplicate",
        ),
        (
            &SMALL_FONT.replace("kern 41 3f", "kern 41 42"),
            "draft.font.glyph_unknown",
        ),
        (
            &SMALL_FONT.replace("l 1 4 3 4 3 2", "l 1 4 5 4"),
            "draft.font.glyph_out_of_bounds",
        ),
        (
            &SMALL_FONT.replace("a 3 2 1 0 360", "a 3 2 4 0 360"),
            "draft.font.glyph_out_of_bounds",
        ),
        (
            &SMALL_FONT.replace("a 3 2 1 0 360", "a 3 2 1 0 0"),
            "draft.font.syntax",
        ),
        (
            &SMALL_FONT.replace("l 1 4 3 4 3 2", "l 1 4 3"),
            "draft.font.syntax",
        ),
        (
            &SMALL_FONT.replace("end\nkern", "kern"),
            "draft.font.syntax",
        ),
        (
            &SMALL_FONT.replace("units 10", "units 1e1"),
            "draft.font.header_invalid",
        ),
        (
            &SMALL_FONT.replace("descender -2", "descender 2"),
            "draft.font.header_invalid",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(code(text), expected, "{text}");
    }
    let tight = Limits {
        input_bytes: 10,
        ..Limits::DEFAULT
    };
    assert_eq!(
        StrokeFont::parse(SMALL_FONT.as_bytes(), &tight)
            .unwrap_err()
            .code(),
        "draft.font.limit_exceeded"
    );
    let long = format!(
        "{}\nglyph 42 1\nl {}\nend\n",
        SMALL_FONT,
        "0 0 ".repeat(300)
    );
    assert_eq!(code(&long), "draft.font.limit_exceeded");
    assert_eq!(code("arc-stroke-font 1\n\u{0}\u{ff}"), "draft.font.syntax");
    assert_eq!(
        StrokeFont::parse(&[0xff, 0xfe], &Limits::DEFAULT)
            .unwrap_err()
            .code(),
        "draft.font.syntax"
    );
}

/// The blocks the shipped font covers completely, and the further characters it has.
const COMPLETE: &[(u32, u32)] = &[
    (0x20, 0x7e),   // Basic Latin
    (0xc0, 0xff),   // Latin-1 letters, with × and ÷
    (0x391, 0x3a1), // Greek capitals (U+03A2 is unassigned)
    (0x3a3, 0x3a9),
    (0x3b1, 0x3c9), // Greek lowercase with final sigma
    (0x410, 0x44f), // Cyrillic basic capitals and lowercase
];
const FURTHER: &[u32] = &[
    0xa0, 0xa1, 0xa2, 0xa3, 0xa5, 0xa6, 0xa8, 0xa9, 0xab, 0xac, 0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb7, 0xb8, 0xb9, 0xbb, 0xbf, 0x11e, 0x11f, 0x130, 0x131, 0x15e, 0x15f, 0x401, 0x451,
    0x2013, 0x2014, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2026, 0x2032, 0x2033, 0x2104, 0x2220,
    0x221e, 0x2248, 0x2264, 0x2265, 0x2300, 0xfffd,
];

#[test]
fn the_shipped_font_covers_its_list_and_stays_inside_its_bounds() {
    let font = builtin().unwrap();
    assert_eq!(font.name, BUILTIN_NAME);
    let mut expected: BTreeSet<char> = COMPLETE
        .iter()
        .flat_map(|(a, b)| *a..=*b)
        .chain(FURTHER.iter().copied())
        .filter_map(char::from_u32)
        .collect();
    expected.remove(&'\u{3a2}');
    let actual: BTreeSet<char> = font.glyphs.keys().copied().collect();
    assert_eq!(actual, expected);
    // Turkish letters, drafting symbols and the missing glyph.
    for c in "çÇğĞıİöÖşŞüÜ⌀°±℄".chars() {
        assert!(font.glyph(c).is_some(), "{c}");
    }
    assert_eq!(font.missing, Some('\u{fffd}'));
    font.check_bounds().unwrap();
    for glyph in font.glyphs.values() {
        assert!(glyph.advance > 0.0);
    }
    // The canonical text reads back to the same font.
    let again = StrokeFont::parse(font.to_text().as_bytes(), &Limits::DEFAULT).unwrap();
    assert_eq!(again, *font);
}

/// The reference model of resolution: availability by lowercase name, document then user
/// table, then the default.
fn reference(
    requested: &str,
    available: &BTreeSet<String>,
    document: &[(String, String)],
    user: &[(String, String)],
) -> (String, Option<SubstitutedBy>) {
    let has = |n: &str| available.contains(&n.to_lowercase());
    if has(requested) {
        return (requested.to_owned(), None);
    }
    for (table, by) in [
        (document, SubstitutedBy::Document),
        (user, SubstitutedBy::User),
    ] {
        let hit = table
            .iter()
            .rev()
            .find(|(r, _)| r.to_lowercase() == requested.to_lowercase());
        if let Some((_, s)) = hit
            && has(s)
        {
            return (s.clone(), Some(by));
        }
    }
    (BUILTIN_NAME.to_owned(), Some(SubstitutedBy::Default))
}

proptest! {
    #[test]
    fn substitution_resolution_matches_the_reference_model(
        names in proptest::collection::btree_set("[a-dA-D]{1,2}", 0..5),
        document in proptest::collection::vec(("[a-dA-D]{1,2}", "[a-dA-D]{1,2}"), 0..6),
        user in proptest::collection::vec(("[a-dA-D]{1,2}", "[a-dA-D]{1,2}"), 0..6),
        requested in "[a-dA-D]{1,2}",
    ) {
        let builtin = builtin().unwrap();
        let mut registry = FontRegistry::with_builtin().unwrap();
        for n in &names {
            registry.add_stroke(n, Arc::clone(&builtin));
        }
        let mut available: BTreeSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        available.insert(BUILTIN_NAME.to_lowercase());
        let fonts = Fonts {
            provider: Arc::new(registry),
            user: Substitutions::new(user.iter().map(|(a, b)| (a.as_str(), b.as_str()))),
            ..Fonts::default()
        };
        let table = Substitutions::new(document.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        let got = resolve(Some(&requested), &table, &fonts).unwrap();
        let (name, by) = reference(&requested, &available, &document, &user);
        prop_assert_eq!(&got.name, &name);
        prop_assert_eq!(got.substitution.as_ref().map(|s| s.by), by);
        if let Some(s) = &got.substitution {
            prop_assert_eq!(&s.requested, &requested);
            prop_assert_eq!(&s.used, &name);
        }
    }
}

#[test]
fn layout_lists_substitutions_and_resolution_needs_the_default() {
    let p = vec![Paragraph {
        runs: vec![Run {
            text: "x".into(),
            font: Some("Unknown Run Font".into()),
            ..Run::default()
        }],
        ..Paragraph::default()
    }];
    let style = TextStyle {
        font: Some("Old Drafting".into()),
        ..TextStyle::default()
    };
    let input = TextInput {
        paragraphs: &p,
        height: 1.0,
        width: 0.0,
        halign: HAlign::Left,
        valign: VAlign::Baseline,
        columns: Columns::default(),
        style: &style,
    };
    let table = Substitutions::new([("old drafting", BUILTIN_NAME)]);
    let out = layout(&input, &table, &Fonts::default(), &options()).unwrap();
    assert_eq!(
        out.substitutions,
        vec![
            Substitution {
                requested: "Old Drafting".into(),
                used: BUILTIN_NAME.into(),
                by: SubstitutedBy::Document,
            },
            Substitution {
                requested: "Unknown Run Font".into(),
                used: BUILTIN_NAME.into(),
                by: SubstitutedBy::Default,
            },
        ]
    );
    let empty = Fonts {
        provider: Arc::new(FontRegistry::default()),
        ..Fonts::default()
    };
    assert_eq!(
        layout(&input, &table, &empty, &options())
            .unwrap_err()
            .code(),
        "draft.font.default_missing"
    );
    let bad = TextInput {
        height: f64::NAN,
        ..input.clone()
    };
    assert_eq!(
        layout(&bad, &table, &Fonts::default(), &options())
            .unwrap_err()
            .code(),
        "draft.text.content_invalid"
    );
    let tight = LayoutOptions {
        max_items: 0,
        ..options()
    };
    assert_eq!(
        layout(&input, &table, &Fonts::default(), &tight)
            .unwrap_err()
            .code(),
        "draft.display.too_large"
    );
    // Extreme heights either lay out or fail with a code.
    for height in [1e-300, 1e300] {
        let extreme = TextInput {
            height,
            ..input.clone()
        };
        let _ = layout(&extreme, &table, &Fonts::default(), &options());
    }
}

fn s(id: ObjectId) -> String {
    id.to_string()
}

#[test]
fn text_commands_validate_and_survive_a_package_round_trip() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "notes"}),
    );
    let single = f.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "Ölçek 1:50", "position": [1.0, 2.0],
               "style": s(style), "halign": "center", "valign": "middle"}),
    );
    let rich = f.create(
        "core.draft.rich_text.create",
        json!({"owner": s(space), "text": "Birinci satır\r\nİkinci\n", "position": [0.0, 0.0],
               "width": 50.0, "attachment": "bottom_right", "columns": 2}),
    );
    let paragraphs = arc_draft::text::paragraphs_of(&f.values(rich));
    assert_eq!(paragraphs.len(), 3);
    assert_eq!(paragraphs[1].runs[0].text, "İkinci");
    f.run(
        "core.draft.rich_text.set",
        json!({"objects": [s(rich)], "paragraphs": [
            {"alignment": "justify", "tabs": [5.0], "runs": [
                {"text": "a\tb", "font": "Arc Stroke", "bold": true, "color": [1.0, 0.0, 0.0, 1.0]},
                {"text": "1", "stack": "diagonal", "denominator": "2"}
            ]}
        ]}),
    )
    .unwrap();
    assert_eq!(
        arc_draft::text::paragraphs_of(&f.values(rich))[0]
            .runs
            .len(),
        2
    );
    // Invalid content is refused by the command and by the type validator.
    let refused = [
        json!({"objects": [s(rich)], "paragraphs": [{"runs": [{"text": "1", "stack": "horizontal"}]}]}),
        json!({"objects": [s(rich)], "paragraphs": [{"tabs": [2.0, 1.0]}]}),
        json!({"objects": [s(rich)], "text": "a", "paragraphs": []}),
    ];
    for params in refused {
        assert!(f.run("core.draft.rich_text.set", params).is_err());
    }
    assert!(
        f.run(
            "core.draft.text.create",
            json!({"owner": s(space), "content": "a\nb", "position": [0.0, 0.0]}),
        )
        .is_err()
    );
    f.run(
        "core.draft.font.substitute",
        json!({"requested": "Old Drafting", "substitute": "Arc Stroke"}),
    )
    .unwrap();
    f.run(
        "core.draft.font.substitute",
        json!({"requested": "Other", "substitute": "Arc Stroke"}),
    )
    .unwrap();
    f.run("core.draft.font.substitute", json!({"requested": "other"}))
        .unwrap();
    let table = document_table(&f.document.snapshot());
    assert_eq!(table.len(), 1);
    assert_eq!(table.get("OLD DRAFTING"), Some("Arc Stroke"));
    // The style in use cannot be deleted.
    assert!(
        f.run("core.draft.style.delete", json!({"objects": [s(style)]}))
            .is_err()
    );
    let _ = single;
    support::round_trip(&f);
}

/// A document whose display evaluators record the entity of every run.
fn counting() -> (Fixture, Arc<Mutex<Vec<ObjectId>>>) {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let mut evaluators = Evaluators::new();
    for key in types::ENTITIES {
        let inner = display::evaluator(DisplayOptions::default());
        let runs = Arc::clone(&runs);
        evaluators
            .register_object(
                TypeKey::new(*key).unwrap(),
                DerivedKey::new(display::DISPLAY).unwrap(),
                move |ctx| {
                    runs.lock().unwrap().push(ctx.object_id().unwrap());
                    inner(ctx)
                },
            )
            .unwrap();
    }
    (Fixture::with_evaluators(evaluators), runs)
}

#[test]
fn text_display_lists_regenerate_with_their_text_style_and_substitution_table() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "notes"}),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "font",
        json!("Old Drafting"),
    );
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "Aç", "position": [10.0, 0.0], "style": s(style),
               "height": 2.0}),
    );
    let rich = f.create(
        "core.draft.rich_text.create",
        json!({"owner": s(space), "paragraphs": [{"runs": [
            {"text": "x", "color": [0.0, 1.0, 0.0, 1.0]}]}], "position": [0.0, 0.0]}),
    );
    let line = f.line(space);
    let regenerate = |f: &mut Fixture| -> BTreeSet<ObjectId> {
        runs.lock().unwrap().clear();
        let snapshot = f.document.snapshot();
        let list = space_list(
            &snapshot,
            Lists::Graph(f.document.graph()),
            space,
            &DisplayOptions::default(),
        )
        .unwrap();
        assert!(list.failed.is_empty(), "{:?}", list.failed);
        runs.lock().unwrap().iter().copied().collect()
    };
    assert_eq!(regenerate(&mut f), BTreeSet::from([text, rich, line]));
    assert!(regenerate(&mut f).is_empty());
    // The text draws strokes near its position; the run color has the source `Object`.
    let snapshot = f.document.snapshot();
    let list = space_list(&snapshot, Lists::Direct, space, &DisplayOptions::default()).unwrap();
    let strokes = list.primitives.iter().filter(|p| p.source == text).count();
    assert!(strokes >= 4);
    for p in list.primitives.iter().filter(|p| p.source == text) {
        let Shape::Stroke { curve, drawn } = &p.shape else {
            panic!("{p:?}")
        };
        assert!(drawn.is_none());
        let bb = curve.bounding_box().unwrap();
        assert!(bb.min().x >= 10.0 - 1e-9 && bb.max().y <= 4.0, "{bb:?}");
    }
    let green = list.primitives.iter().find(|p| p.source == rich).unwrap();
    assert_eq!(
        green.color.as_ref().unwrap().source,
        arc_draft::appearance::Source::Object
    );

    f.set(
        "core.draft.style.modify",
        &[style],
        "width_factor",
        json!(2.0),
    );
    assert_eq!(regenerate(&mut f), BTreeSet::from([text]));
    f.run(
        "core.draft.font.substitute",
        json!({"requested": "Old Drafting", "substitute": "Arc Stroke"}),
    )
    .unwrap();
    assert_eq!(regenerate(&mut f), BTreeSet::from([text, rich]));
    f.set("core.draft.entity.modify", &[text], "content", json!("B"));
    assert_eq!(regenerate(&mut f), BTreeSet::from([text]));

    // The layout for plot and export is the one the list draws, with its substitutions.
    let snapshot = f.document.snapshot();
    let out = display::text_layout(&snapshot, text, &DisplayOptions::default()).unwrap();
    assert_eq!(out.substitutions[0].by, SubstitutedBy::Document);
    assert!(display::text_layout(&snapshot, line, &DisplayOptions::default()).is_err());
}

#[test]
fn the_font_validator_lists_substitutions_as_document_diagnostics() {
    let mut f = Fixture::new();
    f.document
        .add_validator(arc_draft::fonts::validator(Fonts::default()));
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "notes"}),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "font",
        json!("Old Drafting"),
    );
    let codes: Vec<String> = f
        .document
        .diagnostics_snapshot()
        .diagnostics
        .iter()
        .map(|d| d.code().to_owned())
        .collect();
    assert_eq!(codes, ["draft.font.substituted"]);
    let names = arc_draft::fonts::substitutions(&f.document.snapshot(), &Fonts::default()).unwrap();
    assert_eq!(names[0].requested, "Old Drafting");
}
