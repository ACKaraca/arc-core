//! Outline text in PDF output: embedded subset fonts, ToUnicode maps, refusals and hostile bytes.

#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

mod common;

use std::sync::Arc;

use arc_base::Color;
use arc_base::math::Transform2;
use arc_draft::fonts::{BUNDLED_OUTLINE_FAMILY, OutlineFont, bundled_outline};
use arc_draft::plot::{ItemKind, PlotStream};
use arc_draft::text_layout::PositionedGlyph;
use arc_io_pdf::{Output, WriteOptions, write_pdf};
use common::{empty, ids, item, pen};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider};

const SAMPLE: &str = "Ğğ Şş İı Öö Üü Çç ab";

/// The bundled font's glyph identifiers of the characters of `text`, in order.
fn gids(font: &OutlineFont, text: &str) -> Vec<u32> {
    let face = FontRef::from_index(&font.data, font.index).unwrap();
    let map = face.charmap();
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| map.map(c).map_or(0, |g| g.to_u32()))
        .collect()
}

/// A page with one outline run of `text` in `font`.
fn page(font: OutlineFont, text: &str) -> PlotStream {
    let glyphs = gids(&font, text)
        .into_iter()
        .enumerate()
        .map(|(i, id)| PositionedGlyph {
            id,
            position: arc_base::math::Point2::new(i as f64 * 2.0, 0.0),
        })
        .collect();
    let mut s = empty(100.0, 100.0);
    s.items = vec![item(
        ids(1)[0],
        ItemKind::Glyphs {
            font,
            size: 3.0,
            glyphs,
            placement: Transform2::identity(),
            pen: pen(Color::BLACK, 0.25),
        },
    )];
    s
}

fn write(stream: &PlotStream) -> Output {
    write_pdf(&[stream], &mut WriteOptions::default()).unwrap()
}

/// The body of indirect object `number` (between `obj` and `endobj`).
fn object(pdf: &[u8], number: usize) -> &[u8] {
    let head = format!("{number} 0 obj\n");
    let at = pdf
        .windows(head.len())
        .position(|w| w == head.as_bytes())
        .unwrap();
    let end = pdf[at..].windows(6).position(|w| w == b"endobj").unwrap();
    &pdf[at + head.len()..at + end]
}

/// The raw stream data of object `number`.
fn stream_of(pdf: &[u8], number: usize) -> &[u8] {
    let body = object(pdf, number);
    let start = body.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
    let end = body.windows(10).rposition(|w| w == b"\nendstream").unwrap();
    &body[start..end]
}

/// The number that follows `key` in the text of `body`, as `key N 0 R`.
fn reference(body: &[u8], key: &str) -> usize {
    let text = String::from_utf8_lossy(body);
    let at = text.find(key).unwrap() + key.len();
    text[at..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// The object number of the font file of the first embedded font, through the composite font.
fn font_file(pdf: &[u8]) -> Vec<u8> {
    let page = object(pdf, 4);
    let font = reference(page, "/F0 ");
    let descendant = reference(object(pdf, font), "/DescendantFonts [");
    let descriptor = reference(object(pdf, descendant), "/FontDescriptor ");
    stream_of(pdf, reference(object(pdf, descriptor), "/FontFile2 ")).to_vec()
}

/// Counts the moves of the outline of glyph `gid` in `font`.
fn moves(font: &FontRef<'_>, gid: u32) -> usize {
    use skrifa::outline::{DrawSettings, OutlinePen};
    struct Count(usize);
    impl OutlinePen for Count {
        fn move_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn line_to(&mut self, _: f32, _: f32) {}
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
        fn close(&mut self) {}
    }
    let mut count = Count(0);
    if let Some(glyph) = font.outline_glyphs().get(GlyphId::new(gid)) {
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
        let _ = glyph.draw(settings, &mut count);
    }
    count.0
}

#[test]
fn outline_text_is_written_as_an_embedded_subset() {
    let font = bundled_outline();
    let out = write(&page(font.clone(), SAMPLE));
    let pdf = out.bytes.as_slice();
    for needle in [
        "/Type0",
        "/Identity-H",
        "/CIDFontType2",
        "/CIDToGIDMap /Identity",
        "/FontFile2",
    ] {
        assert!(
            pdf.windows(needle.len()).any(|w| w == needle.as_bytes()),
            "{needle}"
        );
    }
    assert_eq!(out.report.summary.dropped, 0);
    assert_eq!(out.report.summary.mapped, 1);

    let file = font_file(pdf);
    let subset = FontRef::from_index(&file, 0).unwrap();
    let original = FontRef::from_index(&font.data, 0).unwrap();
    assert!(
        file.len() < font.data.len() / 2,
        "the subset is much smaller"
    );
    let advance = |f: &FontRef<'_>, gid: u32| {
        f.glyph_metrics(Size::unscaled(), LocationRef::default())
            .advance_width(GlyphId::new(gid))
    };
    for gid in gids(&font, SAMPLE) {
        assert!(moves(&subset, gid) > 0, "glyph {gid} keeps its outline");
        assert_eq!(advance(&subset, gid), advance(&original, gid));
    }
    assert_eq!(moves(&subset, 1), 0, "an unused glyph has no outline");
}

#[test]
fn to_unicode_maps_the_turkish_letters() {
    let out = write(&page(bundled_outline(), SAMPLE));
    let pdf = out.bytes.as_slice();
    let descendant = reference(object(pdf, 4), "/F0 ");
    let type0 = object(pdf, descendant);
    let to_unicode = stream_of(pdf, reference(type0, "/ToUnicode "));
    let text = String::from_utf8_lossy(to_unicode);
    for c in "ĞğŞşİıÖöÜüÇç".chars() {
        assert!(text.contains(&format!("<{:04X}>", c as u32)), "{c}");
    }
}

#[test]
fn the_same_input_gives_identical_bytes() {
    let stream = page(bundled_outline(), SAMPLE);
    assert_eq!(write(&stream).bytes, write(&stream).bytes);
}

/// The offset and the directory record of the OS/2 table of the bundled font.
fn os2_record(data: &[u8]) -> (usize, usize) {
    let tables = usize::from(u16::from_be_bytes([data[4], data[5]]));
    (0..tables)
        .find_map(|i| {
            let record = 12 + 16 * i;
            (data.get(record..record + 4)? == b"OS/2").then(|| {
                let at = data.get(record + 8..record + 12)?;
                Some((u32::from_be_bytes(at.try_into().ok()?) as usize, record))
            })
        })
        .flatten()
        .unwrap()
}

/// The bundled font with `bits` set in its OS/2 fsType.
fn with_fs_type_bits(bits: u16) -> OutlineFont {
    let mut data = bundled_outline().data.to_vec();
    let (os2, _) = os2_record(&data);
    let fs_type = u16::from_be_bytes([data[os2 + 8], data[os2 + 9]]) | bits;
    data[os2 + 8..os2 + 10].copy_from_slice(&fs_type.to_be_bytes());
    OutlineFont {
        family: "Restricted".into(),
        data: Arc::from(data),
        index: 0,
    }
}

/// The reasons of the report of one page with `font`, with their outcomes.
fn reasons_of(font: OutlineFont) -> Vec<(String, arc_io_pdf::Outcome)> {
    write(&page(font, SAMPLE))
        .report
        .items
        .iter()
        .map(|i| (i.reason.clone(), i.outcome))
        .collect()
}

#[test]
fn a_font_that_forbids_embedding_is_reported_not_embedded() {
    // Restricted license (0x0002), no subsetting (0x0100) and bitmap only (0x0200) each refuse.
    for bits in [0x0002, 0x0100, 0x0200] {
        let font = with_fs_type_bits(bits);
        let out = write(&page(font, SAMPLE));
        assert!(
            !out.bytes.windows(9).any(|w| w == b"/FontFile2"),
            "{bits:#x}"
        );
        assert_eq!(
            out.report
                .items
                .iter()
                .map(|i| (i.reason.as_str(), i.outcome))
                .collect::<Vec<_>>(),
            [("io.pdf.font_restricted", arc_io_pdf::Outcome::Dropped)],
            "{bits:#x}"
        );
    }
}

#[test]
fn an_os2_table_too_short_for_fstype_is_refused() {
    let mut data = bundled_outline().data.to_vec();
    let (_, record) = os2_record(&data);
    // Shrink the OS/2 table in its directory record to 8 bytes: fsType is at offset 8.
    data[record + 12..record + 16].copy_from_slice(&8_u32.to_be_bytes());
    let font = OutlineFont {
        family: "Short".into(),
        data: Arc::from(data),
        index: 0,
    };
    assert_eq!(
        reasons_of(font),
        [(
            "io.pdf.font_invalid".to_owned(),
            arc_io_pdf::Outcome::Failed
        )]
    );
}

#[test]
fn two_files_under_one_family_are_two_embedded_fonts() {
    let first = bundled_outline();
    let mut padded = first.data.to_vec();
    padded.extend_from_slice(&[0_u8; 16]);
    let second = OutlineFont {
        family: first.family.clone(),
        data: Arc::from(padded),
        index: 0,
    };
    let mut s = page(first.clone(), "ab");
    let other = page(second, "ab");
    s.items.extend(other.items);
    let out = write(&s);
    let fonts = out.bytes.windows(6).filter(|w| *w == b"/Type0").count();
    assert_eq!(fonts, 2);
}

#[test]
fn a_run_without_drawn_glyphs_is_not_counted_as_written() {
    let mut s = page(bundled_outline(), "ab");
    if let Some(ItemKind::Glyphs { glyphs, .. }) = s.items.first_mut().map(|i| &mut i.kind) {
        glyphs.clear();
    }
    let out = write(&s);
    assert_eq!(out.report.summary.mapped, 0);
    assert!(out.report.items.is_empty());
}

#[test]
fn truncated_and_garbage_font_bytes_are_loss_items() {
    let font = bundled_outline();
    let mut cases: Vec<Vec<u8>> = vec![vec![], vec![0, 1, 0, 0], font.data[..300].to_vec()];
    cases.push(
        (0..4096_u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect(),
    );
    for data in cases {
        let mut stream = page(bundled_outline(), "ab");
        if let Some(ItemKind::Glyphs { font, glyphs, .. }) =
            stream.items.first_mut().map(|i| &mut i.kind)
        {
            *font = OutlineFont {
                family: BUNDLED_OUTLINE_FAMILY.into(),
                data: Arc::from(data),
                index: 0,
            };
            *glyphs = vec![PositionedGlyph {
                id: 1,
                position: arc_base::math::Point2::origin(),
            }];
        }
        let out = write(&stream);
        assert!(!out.bytes.windows(9).any(|w| w == b"/FontFile2"));
        assert_eq!(out.report.items.len(), 1);
        assert_eq!(out.report.items[0].reason, "io.pdf.font_invalid");
    }
}
