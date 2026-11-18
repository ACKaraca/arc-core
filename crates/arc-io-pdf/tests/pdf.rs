//! PDF output: determinism, structure read back with a minimal parser, limits and hostile input.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use arc_base::{Cancel, Color, Limits};
use arc_draft::plot::PlotStream;
use arc_geom::Curve2;
use arc_io_pdf::{Output, WriteOptions, write_pdf};
use common::{
    assert_report_valid, empty, glyphs, ids, nurbs, p, pen, reasons, ring, scene, segment, stroke,
};

fn write(streams: &[&PlotStream]) -> Output {
    let out = write_pdf(streams, &mut WriteOptions::default()).unwrap();
    assert_report_valid(&out.report);
    out
}

/// The bytes as text with every byte above 0x7f shown as `?`, so offsets stay byte offsets.
fn latin1(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| if b.is_ascii() { char::from(*b) } else { '?' })
        .collect()
}

/// What the minimal parser finds in a file.
struct Parsed {
    text: String,
    /// Object number to the text between `obj` and `endobj`.
    objects: Vec<(usize, String)>,
}

/// Reads the header, follows `startxref` to the table, checks that every entry points at its
/// object and returns the objects.
fn parse(bytes: &[u8]) -> Parsed {
    let text = latin1(bytes);
    assert!(text.starts_with("%PDF-1.7\n%"), "header");
    assert!(text.ends_with("startxref\n".to_owned().as_str()) || text.ends_with("%%EOF\n"));
    let tail = text.rsplit_once("startxref\n").unwrap().1;
    let xref_at: usize = tail.lines().next().unwrap().parse().unwrap();
    let table = &text[xref_at..];
    let mut lines = table.lines();
    assert_eq!(lines.next(), Some("xref"));
    let (first, count) = lines.next().unwrap().split_once(' ').unwrap();
    assert_eq!(first, "0");
    let count: usize = count.parse().unwrap();
    assert_eq!(lines.next(), Some("0000000000 65535 f "));
    let mut objects = Vec::new();
    for n in 1..count {
        let entry = lines.next().unwrap();
        assert_eq!(entry.len(), 19, "entries are 20 bytes with the line end");
        assert!(entry.ends_with("00000 n "));
        let at: usize = entry[..10].parse().unwrap();
        let rest = &text[at..];
        let head = format!("{n} 0 obj\n");
        assert!(
            rest.starts_with(&head),
            "entry {n} points at {:?}",
            &rest[..20.min(rest.len())]
        );
        let body = &rest[head.len()..rest.find("\nendobj\n").unwrap()];
        objects.push((n, body.to_owned()));
    }
    assert!(lines.next().unwrap().starts_with("trailer"));
    let trailer = lines.next().unwrap();
    assert!(trailer.contains(&format!("/Size {count}")) && trailer.contains("/Root 1 0 R"));
    Parsed { text, objects }
}

impl Parsed {
    fn pages(&self) -> Vec<&(usize, String)> {
        self.objects
            .iter()
            .filter(|(_, b)| b.contains("/Type /Page "))
            .collect()
    }

    /// The content stream text of the page `i`.
    fn content(&self, i: usize) -> &str {
        let page = &self.pages()[i].1;
        let n: usize = page
            .rsplit_once("/Contents ")
            .unwrap()
            .1
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let body = &self.objects.iter().find(|(k, _)| *k == n).unwrap().1;
        let length: usize = body
            .strip_prefix("<< /Length ")
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let start = body.find("stream\n").unwrap() + 7;
        let stream = &body[start..start + length];
        assert!(
            body[start + length..].starts_with("endstream"),
            "the length is exact"
        );
        stream
    }
}

#[test]
fn identical_streams_give_identical_bytes_and_no_date_unless_asked() {
    let a = write(&[&scene()]);
    assert_eq!(a, write(&[&scene()]));
    assert!(!latin1(&a.bytes).contains("CreationDate"));
    let mut o = WriteOptions {
        creation_date: Some("2026-01-02T03:04:05Z".into()),
        title: Some("Plan (1)".into()),
        ..WriteOptions::default()
    };
    let dated = write_pdf(&[&scene()], &mut o).unwrap();
    let text = parse(&dated.bytes).text;
    assert!(
        text.contains("/CreationDate (D:20260102030405Z)")
            && text.contains("/Title (Plan \\(1\\))")
    );
    let mut o = WriteOptions {
        title: Some("Plan \u{c7}".into()),
        ..WriteOptions::default()
    };
    assert!(
        latin1(&write_pdf(&[&scene()], &mut o).unwrap().bytes)
            .contains("/Title <FEFF0050006C0061006E002000C7>")
    );
}

#[test]
fn file_structure_pages_and_media_box() {
    let (a, b) = (scene(), empty(210.0, 297.0));
    let parsed = parse(&write(&[&a, &b]).bytes);
    let pages = parsed.pages();
    assert_eq!(pages.len(), 2);
    assert!(
        parsed
            .objects
            .iter()
            .any(|(n, o)| *n == 2 && o.starts_with("<< /Type /Pages /Count 2 /Kids ["))
    );
    // 100 mm = 283.4646 pt, 80 mm = 226.7717 pt; A4 is 595.2756 by 841.8898.
    assert!(
        pages[0].1.contains("/MediaBox [0 0 283.4646 226.7717]"),
        "{}",
        pages[0].1
    );
    assert!(pages[1].1.contains("/MediaBox [0 0 595.2756 841.8898]"));
}

#[test]
fn content_has_strokes_fills_clips_and_a_half_point_line_width() {
    let id = ids(2);
    let mut s = empty(100.0, 80.0);
    s.items = vec![stroke(
        id[0],
        segment(p(10.0, 10.0), p(50.0, 10.0)),
        pen(Color::BLACK, 0.5),
    )];
    let parsed = parse(&write(&[&s]).bytes);
    // 0.5 mm is 1.4173 pt.
    assert!(
        parsed
            .content(0)
            .contains("1.4173 w 0 J 0 j\n28.3465 28.3465 m\n141.7323 28.3465 l\nS\n"),
        "{}",
        parsed.content(0)
    );

    let content = parse(&write(&[&scene()]).bytes);
    let c = content.content(0);
    assert!(
        c.starts_with("0.902 0.902 0.7843 rg\n0 0 283.4646 226.7717 re f\nq\n4 M\n"),
        "paper first"
    );
    // Every kind of operator is present: area clip, stroke, fill, curve, close.
    for op in [
        " re W n\n",
        " m\n",
        " l\n",
        " c\n",
        "h\nf\n",
        "\nS\n",
        "1 0 0 rg\n",
    ] {
        assert!(c.contains(op), "operator {op:?} in {c}");
    }
    // The viewport clip: a q, the clip path, W n, the item, Q.
    assert!(c.contains("q\n0 170.0787 m\n141.7323 170.0787 l\n141.7323 226.7717 l\n0 226.7717 l\n0 170.0787 l\nh\nW n\n"), "{c}");
    // Dashes are two subpaths of one stroke and no dash pattern is set.
    assert!(
        c.contains(
            "28.3465 141.7323 m\n85.0394 141.7323 l\n141.7323 141.7323 m\n198.4252 141.7323 l\nS\n"
        ),
        "{c}"
    );
    assert!(!c.contains(" d\n"));
    assert_eq!(c.matches("q\n").count(), c.matches("Q\n").count());
}

/// Parses the numbers before each `c` and checks the Bezier curves of the arc against its
/// circle, in millimeters.
#[test]
fn arcs_are_beziers_within_the_stated_tolerance() {
    let id = ids(1);
    for (radius, chord) in [(10.0, 0.005), (400.0, 0.005), (10.0, 0.0001)] {
        let mut s = empty(1000.0, 1000.0);
        s.items = vec![stroke(
            id[0],
            Curve2::Circle(arc_geom::Circle2::new(p(500.0, 500.0), radius).unwrap()),
            pen(Color::BLACK, 0.5),
        )];
        let mut o = WriteOptions {
            chord_mm: chord,
            ..WriteOptions::default()
        };
        let out = write_pdf(&[&s], &mut o).unwrap();
        assert_report_valid(&out.report);
        let parsed = parse(&out.bytes);
        let k = 72.0 / 25.4;
        let mut cursor = (0.0, 0.0);
        let mut worst = 0.0f64;
        let mut curves = 0;
        for line in parsed.content(0).lines() {
            let n: Vec<f64> = line.split(' ').filter_map(|t| t.parse().ok()).collect();
            if line.ends_with(" m") {
                cursor = (n[0], n[1]);
            } else if line.ends_with(" c") {
                curves += 1;
                let c = [cursor, (n[0], n[1]), (n[2], n[3]), (n[4], n[5])];
                for i in 0..=16 {
                    let t = f64::from(i) / 16.0;
                    let u = 1.0 - t;
                    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                    let x: f64 = (0..4).map(|j| w[j] * c[j].0).sum();
                    let y: f64 = (0..4).map(|j| w[j] * c[j].1).sum();
                    let d = ((x / k - 500.0).powi(2) + (y / k - 500.0).powi(2)).sqrt();
                    worst = worst.max((d - radius).abs());
                }
                cursor = c[3];
            }
        }
        // The file stores four decimals of a point, 1.4e-5 mm.
        assert!(
            worst <= chord + 5e-5,
            "radius {radius} chord {chord}: worst {worst}"
        );
        assert!(curves >= 4 && (chord > 0.001 || curves > 4));
        assert!(reasons(&out.report).contains(&"io.pdf.arcs_approximated"));
    }
}

#[test]
fn layers_are_optional_content_groups() {
    let id = ids(3);
    let mut s = empty(100.0, 100.0);
    let mut a = stroke(
        id[0],
        segment(p(1.0, 1.0), p(9.0, 9.0)),
        pen(Color::BLACK, 0.5),
    );
    let mut b = stroke(
        id[1],
        segment(p(2.0, 1.0), p(9.0, 5.0)),
        pen(Color::BLACK, 0.5),
    );
    let c = stroke(
        id[1],
        segment(p(3.0, 1.0), p(9.0, 4.0)),
        pen(Color::BLACK, 0.5),
    );
    a.layer = Some(id[2]);
    b.layer = Some(id[0]);
    s.items = vec![a, b, c];
    let out = write(&[&s]);
    let parsed = parse(&out.bytes);
    let catalog = &parsed.objects[0].1;
    assert!(catalog.contains("/OCProperties << /OCGs [4 0 R 5 0 R] /D << /Order [4 0 R 5 0 R] /ON [4 0 R 5 0 R] >> >>"), "{catalog}");
    assert!(parsed.objects[3].1.starts_with("<< /Type /OCG /Name ("));
    let content = parsed.content(0);
    // Layers sort by identifier (id[0] < id[2]); the unlayered item is outside the groups.
    assert!(content.contains("/OC /L1 BDC\n") && content.contains("/OC /L0 BDC\n"));
    assert_eq!(content.matches("BDC").count(), 2);
    assert_eq!(content.matches("EMC").count(), 2);
    assert!(
        parsed.pages()[0]
            .1
            .contains("/Properties << /L0 4 0 R /L1 5 0 R >>")
    );
    assert!(!reasons(&out.report).contains(&"io.pdf.layers_dropped"));
}

#[test]
fn transparency_uses_an_extended_graphics_state() {
    let id = ids(1);
    let mut s = empty(50.0, 50.0);
    s.items = vec![stroke(
        id[0],
        segment(p(1.0, 1.0), p(9.0, 9.0)),
        pen(Color::new(0.0, 0.0, 0.0, 0.5), 0.5),
    )];
    let parsed = parse(&write(&[&s]).bytes);
    assert!(
        parsed.pages()[0]
            .1
            .contains("/ExtGState << /A128 << /Type /ExtGState /CA 0.502 /ca 0.502 >> >>")
    );
    assert!(parsed.content(0).contains("q\n/A128 gs\n"));
}

#[test]
fn glyph_runs_nurbs_and_failures_are_reported() {
    let id = ids(3);
    let mut s = empty(100.0, 100.0);
    s.items = vec![
        glyphs(id[0]),
        stroke(id[1], nurbs(), pen(Color::BLACK, 0.5)),
    ];
    s.failed = vec![id[2]];
    let out = write(&[&s]);
    assert!(out.report.has_loss());
    assert_eq!(
        reasons(&out.report),
        [
            "io.pdf.glyphs_dropped",
            "io.pdf.nurbs_flattened",
            "io.pdf.sources_failed"
        ]
    );
    assert_eq!(out.report.header.format, "pdf");
}

#[test]
fn nothing_lost_still_gives_a_report() {
    let out = write(&[&empty(10.0, 10.0)]);
    assert!(out.report.items.is_empty() && !out.report.has_loss());
}

#[test]
fn limits_cancellation_and_progress() {
    let s = scene();
    let case = |limits: Limits| {
        let mut o = WriteOptions {
            limits,
            ..WriteOptions::default()
        };
        let e = write_pdf(&[&s], &mut o).unwrap_err();
        (e.code(), format!("{:?}", e.arguments().get("limit")))
    };
    assert_eq!(
        case(Limits {
            record_count: 3,
            ..Limits::CONSTRAINED
        }),
        ("io.limit_exceeded", "Some(Text(\"record_count\"))".into())
    );
    assert_eq!(
        case(Limits {
            input_bytes: 300,
            ..Limits::CONSTRAINED
        }),
        ("io.limit_exceeded", "Some(Text(\"input_bytes\"))".into())
    );
    // The final check also covers the structure around the content streams.
    let size = write(&[&s]).bytes.len() as u64;
    assert_eq!(
        case(Limits {
            input_bytes: size - 1,
            ..Limits::CONSTRAINED
        })
        .0,
        "io.limit_exceeded"
    );
    let mut ok = WriteOptions {
        limits: Limits {
            input_bytes: size,
            ..Limits::CONSTRAINED
        },
        ..WriteOptions::default()
    };
    write_pdf(&[&s], &mut ok).unwrap();

    let cancel = Cancel::new();
    cancel.cancel();
    let mut o = WriteOptions {
        cancel,
        ..WriteOptions::default()
    };
    assert_eq!(
        write_pdf(&[&s], &mut o).unwrap_err().code(),
        "base.cancelled"
    );

    let mut seen = Vec::new();
    let mut record = |p: arc_io_pdf::Progress| seen.push((p.items_done, p.items_total));
    let mut o = WriteOptions {
        progress: Some(&mut record),
        ..WriteOptions::default()
    };
    write_pdf(&[&s, &s], &mut o).unwrap();
    assert_eq!(seen.first(), Some(&(0, 16)));
    assert_eq!(seen.last(), Some(&(16, 16)));
}

#[test]
fn hostile_options_and_streams_are_rejected_with_their_codes() {
    let s = scene();
    let code = |streams: &[&PlotStream], o: &mut WriteOptions<'_>| {
        write_pdf(streams, o).unwrap_err().code()
    };
    assert_eq!(
        code(&[], &mut WriteOptions::default()),
        "io.pdf.option_invalid"
    );
    let mut o = WriteOptions {
        chord_mm: f64::NAN,
        ..WriteOptions::default()
    };
    assert_eq!(code(&[&s], &mut o), "io.pdf.option_invalid");
    for date in [
        "",
        "2026-01-02",
        "2026-01-02T03:04:05",
        "2026-00-02T03:04:05Z",
        "2026-01-02T24:04:05Z",
        "2026-01-02T03:04:0\u{e9}Z",
        "20x6-01-02T03:04:05Z",
    ] {
        let mut o = WriteOptions {
            creation_date: Some(date.into()),
            ..WriteOptions::default()
        };
        assert_eq!(code(&[&s], &mut o), "io.pdf.option_invalid", "{date:?}");
    }
    // A page beyond 14400 points (5080 mm) is refused; one inside is written.
    let huge = empty(6000.0, 100.0);
    assert_eq!(
        code(&[&huge], &mut WriteOptions::default()),
        "io.pdf.stream_invalid"
    );
    let mut far = scene();
    far.items[0].kind = arc_draft::plot::ItemKind::Stroke {
        curve: segment(p(0.0, 0.0), p(f64::MAX, 0.0)),
        pieces: None,
        pen: pen(Color::BLACK, 1.0),
    };
    assert_eq!(
        code(&[&far], &mut WriteOptions::default()),
        "io.pdf.stream_invalid"
    );
}

mod extreme {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Arcs of any magnitude either write or fail with a registered code; they never panic.
        #[test]
        fn arcs_of_any_magnitude_never_panic(
            exp in -300i32..300,
            mantissa in 1.0f64..10.0,
            sweep in 0.001f64..6.0,
            start in 0.0f64..6.0,
            cx in -1e3f64..1e3,
        ) {
            let radius = mantissa * 10f64.powi(exp);
            let Ok(arc) = arc_geom::Arc2::new(p(cx, 0.0), radius, start, sweep) else { return Ok(()); };
            let mut s = empty(100.0, 100.0);
            s.items = vec![stroke(ids(1)[0], Curve2::Arc(arc), pen(Color::BLACK, 0.5))];
            for result in [
                write_pdf(&[&s], &mut WriteOptions::default()),
                arc_io_pdf::write_svg(&s, &mut WriteOptions::default()),
            ] {
                match result {
                    Ok(out) => assert_report_valid(&out.report),
                    Err(e) => prop_assert!(arc_io_pdf::codes::ALL.iter().any(|c| c.code == e.code())),
                }
            }
        }
    }
}

#[test]
fn region_fill_uses_the_even_odd_operator_with_the_hole_as_beziers() {
    let parsed = parse(&write(&[&ring()]).bytes);
    let c = parsed.content(0);
    assert_eq!(c.matches("f*\n").count(), 1, "{c}");
    // Outer loop and hole are two subpaths of the one fill; the hole is curves, not lines.
    assert_eq!(c.matches("h\n").count(), 2, "{c}");
    assert!(c.contains(" c\n"), "{c}");
    assert_eq!(c, parse(&write(&[&ring()]).bytes).content(0));
}
