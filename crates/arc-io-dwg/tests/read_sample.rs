//! Reading drawings written by the wrapped library into the neutral model.

#![allow(clippy::panic, clippy::unwrap_used, reason = "tests")]

mod common;

use acadrust::types::DxfVersion;
use arc_io_dwg::read;
use arc_io_dxf::ReadOptions;
use arc_io_dxf::model::{EntityKind, HatchBoundary};

fn read_sample(version: DxfVersion) -> arc_io_dxf::ReadOutput {
    let bytes = common::dwg_bytes(&common::sample_document(version));
    match read(&bytes, &mut ReadOptions::default()) {
        Ok(output) => output,
        Err(err) => panic!("read failed: {} {err:?}", err.code()),
    }
}

#[test]
fn sample_entities_map_to_the_neutral_model() {
    let output = read_sample(DxfVersion::AC1032);
    let drawing = &output.drawing;
    assert_eq!(drawing.version, arc_io_dxf::Version::R2018);

    let names: Vec<&str> = drawing
        .entities
        .iter()
        .map(|e| e.kind.type_name())
        .collect();
    assert_eq!(
        names,
        [
            "LINE",
            "ARC",
            "TEXT",
            "LWPOLYLINE",
            "MTEXT",
            "INSERT",
            "HATCH"
        ]
    );

    let line = &drawing.entities[0];
    assert_eq!(line.props.layer, "WALLS");
    let EntityKind::Line(line) = &line.kind else {
        panic!("first entity is not a line");
    };
    assert_eq!(line.start, [0.0, 0.0, 0.0]);
    assert_eq!(line.end, [10.0, 5.0, 0.0]);

    // The library stores arc angles in radians; the model keeps degrees.
    let EntityKind::Arc(arc) = &drawing.entities[1].kind else {
        panic!("second entity is not an arc");
    };
    assert!(
        (arc.start_angle - 30.0).abs() < 1e-9,
        "start {}",
        arc.start_angle
    );
    assert!((arc.end_angle - 90.0).abs() < 1e-9, "end {}", arc.end_angle);
    assert!((arc.radius - 3.0).abs() < 1e-12);

    let EntityKind::Text(text) = &drawing.entities[2].kind else {
        panic!("third entity is not text");
    };
    assert_eq!(text.value, "ROOM");
    assert!((text.height - 2.5).abs() < 1e-12);

    let EntityKind::LwPolyline(poly) = &drawing.entities[3].kind else {
        panic!("fourth entity is not an lwpolyline");
    };
    assert_eq!(poly.vertices.len(), 3);
    assert_eq!(poly.flags & 1, 1, "closed flag");

    let EntityKind::MText(mtext) = &drawing.entities[4].kind else {
        panic!("fifth entity is not mtext");
    };
    assert_eq!(mtext.text, "NOTE");

    let EntityKind::Insert(insert) = &drawing.entities[5].kind else {
        panic!("sixth entity is not an insert");
    };
    assert_eq!(insert.block, "DOOR");
    assert_eq!(insert.attribs.len(), 1);
    match &insert.attribs[0].kind {
        EntityKind::Attrib(attrib) => assert_eq!(attrib.tag, "NUMBER"),
        other => panic!("attribute maps to {}", other.type_name()),
    }
}

#[test]
fn block_definitions_and_layers_are_carried() {
    let output = read_sample(DxfVersion::AC1032);
    let drawing = &output.drawing;

    assert_eq!(drawing.blocks.len(), 1);
    let block = &drawing.blocks[0];
    match &block.begin.kind {
        EntityKind::Block(begin) => assert_eq!(begin.name, "DOOR"),
        other => panic!("block begins with {}", other.type_name()),
    }
    let circle = block.entities.iter().find_map(|e| match &e.kind {
        EntityKind::Circle(c) => Some(c),
        _ => None,
    });
    assert!((circle.map_or(f64::NAN, |c| c.radius) - 1.0).abs() < 1e-12);

    let layers = drawing.tables.layers.as_ref().map(|t| {
        t.entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
    });
    assert!(layers.is_some_and(|names| names.contains(&"WALLS")));
}

#[test]
fn dropped_and_partial_data_is_counted() {
    let output = read_sample(DxfVersion::AC1032);
    let report = &output.report;
    let item = |reason: &str| report.items.iter().find(|i| i.reason == reason);

    let header = item("io.dwg.header_dropped").map(|i| i.arguments.get("count").cloned());
    assert_eq!(header, Some(Some(arc_io_dxf::loss::Argument::Int(1))));
    assert!(
        item("io.dwg.object_dropped").is_some(),
        "the objects of the file are reported"
    );
    assert!(
        item("io.dwg.table_dropped").is_some(),
        "the view ports are reported"
    );
    assert_eq!(
        report.summary.mapped, 8,
        "seven top-level entities and the circle of the block"
    );
    let solid = item("io.dwg.entity_dropped").filter(|i| i.record_kind.as_deref() == Some("SOLID"));
    assert!(solid.is_some(), "the solid is reported as dropped");
    assert_eq!(report.header.format, "dwg");
    assert!(report.header.limits.is_some());
}

#[test]
fn every_supported_version_reads_back() {
    for version in [
        DxfVersion::AC1012,
        DxfVersion::AC1014,
        DxfVersion::AC1015,
        DxfVersion::AC1018,
        DxfVersion::AC1021,
        DxfVersion::AC1024,
        DxfVersion::AC1027,
        DxfVersion::AC1032,
    ] {
        let bytes = common::dwg_bytes(&common::sample_document(version));
        let result = read(&bytes, &mut ReadOptions::default());
        assert!(
            result.is_ok(),
            "{version:?}: {:?}",
            result.err().map(|e| e.code())
        );
    }
}

#[test]
fn hatch_boundary_is_mapped_as_polyline() {
    let output = read_sample(DxfVersion::AC1032);
    let hatch = output.drawing.entities.iter().find_map(|e| match &e.kind {
        EntityKind::Hatch(h) => Some(h),
        _ => None,
    });
    let Some(hatch) = hatch else {
        panic!("the hatch is mapped");
    };
    assert_eq!(hatch.loops.len(), 1);
    match &hatch.loops[0].boundary {
        HatchBoundary::Polyline {
            closed, vertices, ..
        } => {
            assert!(*closed);
            assert_eq!(vertices.len(), 3);
        }
        HatchBoundary::Edges(_) => panic!("a single polyline path is kept in polyline form"),
    }
}
