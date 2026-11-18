//! Reading the hand-written files under `tests/data`, the version matrix, ASCII against binary,
//! and writing them back.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "tests"
)]

mod common;

use arc_io_dxf::model::{HatchBoundary, HatchEdge};
use arc_io_dxf::{
    Drawing, EntityKind, ObjectKind, ReadOptions, Value, Version, WriteOptions, read, write,
};
use common::{assert_report_valid, golden, to_binary};

fn load(bytes: &[u8]) -> Drawing {
    let out = read(bytes, &mut ReadOptions::default()).unwrap();
    assert_report_valid(&out.report);
    out.drawing
}

/// Every golden file with the version it declares.
const MATRIX: &[(&str, Version)] = &[
    // No header at all: read as the oldest version.
    ("no-version.dxf", Version::R12),
    // R12: no handles or markers, Turkish code page, POLYLINE and INSERT sequences, a comment.
    ("r12.dxf", Version::R12),
    // R13, R14, 2004, 2010, 2013: one or two simple entities with handles and markers.
    ("r13.dxf", Version::R13),
    ("r14.dxf", Version::R14),
    // 2000: classes, tables, model-space block, LWPOLYLINE, ELLIPSE, SPLINE, extended data,
    // application groups, dictionaries and an unknown object.
    ("r2000.dxf", Version::R2000),
    ("r2004.dxf", Version::R2004),
    // 2007: UTF-8 multi-line text in two chunks with a Unicode escape.
    ("r2007.dxf", Version::R2007),
    ("r2010.dxf", Version::R2010),
    ("r2013.dxf", Version::R2013),
    // 2018: HATCH with every edge kind and fit data, DIMENSION, LEADER, VIEWPORT, IMAGE, SOLID,
    // an unknown entity, LAYOUT, XRECORD, GROUP and an unknown section.
    ("r2018.dxf", Version::R2018),
];

#[test]
fn version_matrix_reads_every_file() {
    for (name, version) in MATRIX {
        let d = load(&golden(name));
        assert_eq!(d.version, *version, "{name}");
        assert!(!d.entities.is_empty(), "{name}");
        assert!(d.header_variable("$ACADVER").is_none());
    }
}

#[test]
fn ascii_and_binary_forms_read_the_same() {
    for (name, _) in MATRIX {
        let ascii = golden(name);
        let expected = load(&ascii);
        for short in [true, false] {
            let binary = to_binary(&ascii, short);
            assert_eq!(load(&binary), expected, "{name} short codes {short}");
        }
    }
}

#[test]
fn r12_content_and_code_page() {
    let out = read(&golden("r12.dxf"), &mut ReadOptions::default()).unwrap();
    let d = out.drawing;
    let layers = d.tables.layers.as_ref().unwrap();
    assert_eq!(layers.entries[0].name, "KAPI");
    assert_eq!(layers.entries[0].data.color, 3);
    let lt = &d.tables.linetypes.as_ref().unwrap().entries[0];
    assert_eq!(lt.data.elements.len(), 2);
    assert_eq!(lt.data.elements[1].length, -0.25);
    assert_eq!(d.blocks.len(), 1);
    assert!(matches!(
        d.blocks[0].entities[0].kind,
        EntityKind::AttDef(_)
    ));
    let EntityKind::Polyline(p) = &d.entities[2].kind else {
        panic!("polyline expected")
    };
    assert_eq!(p.vertices.len(), 2);
    assert!(p.seqend.is_some());
    let EntityKind::Vertex(v) = &p.vertices[0].kind else {
        panic!("vertex expected")
    };
    assert_eq!(v.bulge, Some(0.5));
    let EntityKind::Text(t) = &d.entities[3].kind else {
        panic!("text expected")
    };
    assert_eq!(t.value, "Işık Ğüş");
    let EntityKind::Insert(i) = &d.entities[4].kind else {
        panic!("insert expected")
    };
    assert_eq!(i.attribs.len(), 1);
    assert!(i.seqend.is_some());
    assert!(
        out.report
            .items
            .iter()
            .any(|item| item.reason == "io.dxf.comment_dropped")
    );
}

#[test]
fn r2000_content() {
    let d = load(&golden("r2000.dxf"));
    assert_eq!(d.classes[0].dxf_name, "ACDBDICTIONARYWDFLT");
    let line = &d.entities[0];
    assert_eq!(line.head.handle.map(|h| h.0), Some(0x30));
    assert_eq!(line.head.owner.map(|h| h.0), Some(0x1F));
    assert_eq!(line.head.app_groups.len(), 3);
    assert_eq!(line.props.true_color, Some(0xFF0000));
    let EntityKind::LwPolyline(p) = &line.kind else {
        panic!("lwpolyline expected")
    };
    assert_eq!(p.vertices.len(), 3);
    assert_eq!(p.vertices[0].bulge, Some(1.0));
    assert_eq!(p.vertices[1].start_width, Some(0.5));
    let EntityKind::Spline(s) = &d.entities[2].kind else {
        panic!("spline expected")
    };
    assert_eq!((s.knots.len(), s.control_points.len()), (6, 3));
    assert_eq!(d.entities[1].props.extrusion, Some([0.0, 0.0, 1.0]));
    let text = &d.entities[3];
    let EntityKind::Text(t) = &text.kind else {
        panic!("text expected")
    };
    assert_eq!(t.value, "café ½ Ω");
    assert_eq!(text.xdata[0].app, "ARC_TEST");
    assert_eq!(text.xdata[0].groups.len(), 5);
    let ObjectKind::Dictionary(root) = &d.objects[0].kind else {
        panic!("dictionary expected")
    };
    assert_eq!(root.entries[0].name, "ACAD_GROUP");
    assert!(matches!(d.objects[2].kind, ObjectKind::Preserved(_)));
    assert_eq!(d.objects[2].source_id().map(|h| h.0), Some(0xE));
}

#[test]
fn r2007_content() {
    let d = load(&golden("r2007.dxf"));
    assert_eq!(d.entities[0].props.layer, "Katman_Ç");
    let EntityKind::MText(m) = &d.entities[0].kind else {
        panic!("mtext expected")
    };
    assert_eq!(m.text, "Ağaç ışığı ve şöğüş:İstanbul ☺");
}

#[test]
fn r2018_content() {
    let d = load(&golden("r2018.dxf"));
    let EntityKind::Hatch(h) = &d.entities[0].kind else {
        panic!("hatch expected")
    };
    assert_eq!(h.loops.len(), 2);
    assert!(matches!(
        &h.loops[0].boundary,
        HatchBoundary::Polyline { has_bulge: true, vertices, .. } if vertices[1].bulge == 0.5
    ));
    let HatchBoundary::Edges(edges) = &h.loops[1].boundary else {
        panic!("edges expected")
    };
    assert_eq!(edges.len(), 4);
    let HatchEdge::Spline(s) = &edges[3] else {
        panic!("spline edge expected")
    };
    assert_eq!((s.knots.len(), s.fit_points.len()), (8, 2));
    assert_eq!(s.end_tangent, Some([1.0, 0.0]));
    assert_eq!(h.loops[1].sources[0].0, 0x51);
    assert_eq!(h.pattern_lines[0].dashes, vec![0.125, -0.0625]);
    assert_eq!(h.seeds, vec![[5.0, 5.0]]);
    assert_eq!(d.entities[0].extras[0].code, 450);
    let EntityKind::Dimension(dim) = &d.entities[1].kind else {
        panic!("dimension expected")
    };
    assert_eq!(dim.point14, Some([10.0, 0.0, 0.0]));
    let EntityKind::Leader(l) = &d.entities[2].kind else {
        panic!("leader expected")
    };
    assert_eq!(l.vertices.len(), 2);
    assert_eq!(d.entities[2].extras.len(), 3);
    let EntityKind::Viewport(v) = &d.entities[3].kind else {
        panic!("viewport expected")
    };
    assert_eq!(v.frozen_layers.len(), 2);
    assert!(d.entities[3].props.paper_space);
    let EntityKind::Image(i) = &d.entities[4].kind else {
        panic!("image expected")
    };
    assert_eq!(i.clip_vertices.len(), 2);
    assert!(matches!(&d.entities[5].kind, EntityKind::Preserved(r) if r.kind == "ARC_TEST_SHAPE"));
    let ObjectKind::Layout(layout) = &d.objects[0].kind else {
        panic!("layout expected")
    };
    assert_eq!(layout.name, "Layout1");
    assert_eq!(layout.plot_settings.len(), 3);
    assert_eq!(layout.block_record.map(|h| h.0), Some(0x81));
    let ObjectKind::XRecord(x) = &d.objects[1].kind else {
        panic!("xrecord expected")
    };
    assert_eq!(x.cloning, Some(1));
    assert_eq!(x.data.len(), 3);
    assert_eq!(x.data[1].value, Value::Int(9));
    assert_eq!(d.preserved_sections[0].name, "ARC_TEST_SECTION");
}

fn without_seed(mut d: Drawing) -> Drawing {
    d.header.retain(|v| v.name != "$HANDSEED");
    d
}

#[test]
fn golden_files_survive_a_write_and_reread() {
    for name in ["r2000.dxf", "r2007.dxf", "r2018.dxf"] {
        let original = load(&golden(name));
        for binary in [false, true] {
            let mut options = WriteOptions {
                version: original.version,
                binary,
                ..WriteOptions::default()
            };
            let out = write(&original, &mut options).unwrap();
            assert_report_valid(&out.report);
            let again = load(&out.bytes);
            assert_eq!(
                without_seed(again),
                without_seed(original.clone()),
                "{name}"
            );
        }
    }
}

#[test]
fn writer_is_deterministic_and_idempotent() {
    for (name, _) in MATRIX {
        let d = load(&golden(name));
        for version in Version::WRITABLE {
            for binary in [false, true] {
                let options = || WriteOptions {
                    version,
                    binary,
                    ..WriteOptions::default()
                };
                let first = write(&d, &mut options()).unwrap();
                let second = write(&d, &mut options()).unwrap();
                assert_eq!(first.bytes, second.bytes, "{name} {version:?}");
                assert_report_valid(&first.report);
                // Writing what was read back gives the same bytes again.
                let reread = load(&first.bytes);
                assert_eq!(write(&reread, &mut options()).unwrap().bytes, first.bytes);
            }
        }
    }
}

#[test]
fn r12_written_as_2000_gets_handles_and_reports_them() {
    let d = load(&golden("r12.dxf"));
    let out = write(&d, &mut WriteOptions::for_version(Version::R2000)).unwrap();
    let reasons: Vec<&str> = out.report.items.iter().map(|i| i.reason.as_str()).collect();
    assert!(reasons.contains(&"io.dxf.handle_assigned"));
    assert!(reasons.contains(&"io.dxf.structure_incomplete"));
    let again = load(&out.bytes);
    assert!(again.entities.iter().all(|e| e.head.handle.is_some()));
    let EntityKind::Text(t) = &again.entities[3].kind else {
        panic!("text expected")
    };
    assert_eq!(t.value, "Işık Ğüş");
    // The Turkish letters are written as bytes of code page 1254, not as escapes.
    assert!(out.bytes.windows(2).any(|w| w == [0xFE, 0xFD]));
}

#[test]
fn hatch_fit_data_is_reported_for_targets_before_2010() {
    let d = load(&golden("r2018.dxf"));
    let out = write(&d, &mut WriteOptions::for_version(Version::R2004)).unwrap();
    assert!(out.report.has_loss());
    assert!(
        out.report
            .items
            .iter()
            .any(|i| i.reason == "io.dxf.fit_data_dropped")
    );
    let again = load(&out.bytes);
    let EntityKind::Hatch(h) = &again.entities[0].kind else {
        panic!("hatch expected")
    };
    let HatchBoundary::Edges(edges) = &h.loops[1].boundary else {
        panic!("edges expected")
    };
    assert!(matches!(&edges[3], HatchEdge::Spline(s) if s.fit_points.is_empty()));
}

#[test]
fn unsupported_versions_are_refused() {
    let e = write(
        &Drawing::default(),
        &mut WriteOptions::for_version(Version::R14),
    )
    .unwrap_err();
    assert_eq!(e.code(), "io.unsupported_version");
    let text = b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1006\n0\nENDSEC\n0\nEOF\n";
    let e = read(text, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(e.code(), "io.unsupported_version");
}

#[test]
fn preserved_records_keep_their_groups_through_a_write() {
    let d = load(&golden("r2018.dxf"));
    let out = write(&d, &mut WriteOptions::default()).unwrap();
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(text.contains("ARC_TEST_SHAPE\r\n  5\r\n56\r\n100\r\nAcDbEntity\r\n  8\r\n0\r\n100\r\nArcTestShape\r\n 10\r\n1.0"));
    assert!(text.contains("ARC_TEST_SECTION\r\n  0\r\nITEM\r\n  1\r\nopaque"));
}

#[test]
fn reading_from_a_stream_matches_reading_bytes() {
    let bytes = golden("r2000.dxf");
    let from_stream =
        arc_io_dxf::read_stream(bytes.as_slice(), &mut ReadOptions::default()).unwrap();
    assert_eq!(from_stream.drawing, load(&bytes));
}
