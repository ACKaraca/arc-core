//! Reading drawings written by the wrapped library into the neutral model.

#![allow(clippy::panic, clippy::unwrap_used, reason = "tests")]

mod common;

use acadrust::entities::EntityType;
use acadrust::types::{DxfVersion, Vector2, Vector3};
use arc_base::Limits;
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
    // The summary counts the kept records as the DXF reader does: the entities (seven top-level
    // and the circle of the block), the BLOCK and ENDBLK records, the six symbol tables and every
    // table entry.
    let drawing = &output.drawing;
    let table_entries: usize = [
        drawing.tables.layers.as_ref().map(|t| t.entries.len()),
        drawing.tables.linetypes.as_ref().map(|t| t.entries.len()),
        drawing.tables.styles.as_ref().map(|t| t.entries.len()),
        drawing.tables.appids.as_ref().map(|t| t.entries.len()),
        drawing.tables.dimstyles.as_ref().map(|t| t.entries.len()),
        drawing
            .tables
            .block_records
            .as_ref()
            .map(|t| t.entries.len()),
    ]
    .into_iter()
    .flatten()
    .sum();
    let block_members: usize = drawing.blocks.iter().map(|b| b.entities.len()).sum();
    let kept =
        drawing.entities.len() + block_members + 2 * drawing.blocks.len() + 6 + table_entries;
    assert_eq!(report.summary.mapped, u64::try_from(kept).unwrap_or(0));
    assert_eq!(drawing.entities.len() + block_members, 8);
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

fn read_document(doc: &acadrust::CadDocument) -> arc_io_dxf::ReadOutput {
    let bytes = common::dwg_bytes(doc);
    match read(&bytes, &mut ReadOptions::default()) {
        Ok(output) => output,
        Err(err) => panic!("read failed: {} {err:?}", err.code()),
    }
}

fn table_entry<'a, T>(
    table: &'a Option<arc_io_dxf::model::Table<T>>,
    name: &str,
) -> Option<&'a arc_io_dxf::model::TableEntry<T>> {
    table
        .as_ref()
        .and_then(|t| t.entries.iter().find(|e| e.name == name))
}

#[test]
fn model_space_without_a_layout_handle_stays_top_level() {
    // Records of the oldest versions carry no layout handle. A model space without one must still
    // hold the drawing's entities at the top level, not become a block definition.
    let mut doc = common::sample_document(DxfVersion::AC1012);
    if let Some(record) = doc.block_records.get_mut("*Model_Space") {
        record.layout = acadrust::types::Handle::NULL;
    }
    let output = read_document(&doc);
    let names: Vec<&str> = output
        .drawing
        .entities
        .iter()
        .map(|e| e.kind.type_name())
        .collect();
    assert!(names.contains(&"LINE"), "{names:?}");
    assert!(names.contains(&"LWPOLYLINE"), "{names:?}");
    assert!(
        output
            .drawing
            .blocks
            .iter()
            .all(|b| { b.entities.iter().all(|e| e.kind.type_name() != "LINE") })
    );
}

#[test]
fn layer_that_is_off_keeps_its_state_with_a_by_block_color() {
    let mut doc = common::sample_document(DxfVersion::AC1032);
    let mut layer = acadrust::tables::Layer::new("OFF_BY_BLOCK");
    layer.color = acadrust::types::Color::ByBlock;
    layer.turn_off();
    doc.layers.add_or_replace(layer);
    let mut lit = acadrust::tables::Layer::new("OFF_INDEX");
    lit.color = acadrust::types::Color::Index(3);
    lit.turn_off();
    doc.layers.add_or_replace(lit);
    let output = read_document(&doc);
    let layers = &output.drawing.tables.layers;
    let by_block = table_entry(layers, "OFF_BY_BLOCK").map(|e| e.data.color);
    assert!(by_block.is_some_and(|c| c < 0), "{by_block:?}");
    let indexed = table_entry(layers, "OFF_INDEX").map(|e| e.data.color);
    assert_eq!(indexed, Some(-3));
}

#[test]
fn text_style_oblique_angle_is_kept_in_degrees() {
    let mut doc = common::sample_document(DxfVersion::AC1032);
    let mut style = acadrust::tables::TextStyle::new("OBLIQUE");
    style.oblique_angle = 0.2;
    doc.text_styles.add_or_replace(style);
    let output = read_document(&doc);
    let entry = table_entry(&output.drawing.tables.styles, "OBLIQUE");
    let oblique = entry.map(|e| e.data.oblique);
    assert!(
        oblique.is_some_and(|v| (v - 0.2_f64.to_degrees()).abs() < 1e-9),
        "{oblique:?}"
    );
}

#[test]
fn two_dimensional_polyline_flags_and_smoothing_are_mapped() {
    let mut poly = acadrust::entities::Polyline2D::new();
    poly.flags = acadrust::entities::PolylineFlags::from_bits(2 | 4);
    poly.smooth_surface = acadrust::entities::SmoothSurfaceType::CubicBSpline;
    poly.elevation = 2.5;
    poly.add_vertex(acadrust::entities::Vertex2D::new(Vector3::new(
        0.0, 0.0, 2.5,
    )));
    poly.add_vertex(acadrust::entities::Vertex2D::new(Vector3::new(
        4.0, 0.0, 2.5,
    )));
    let output = read_document(&common::document_with(
        DxfVersion::AC1032,
        EntityType::Polyline2D(poly),
    ));
    let Some(entity) = output.drawing.entities.first() else {
        panic!("the polyline is mapped");
    };
    let EntityKind::Polyline(poly) = &entity.kind else {
        panic!("a two-dimensional polyline is mapped to a polyline");
    };
    assert_eq!(poly.flags, 2 | 4);
    assert_eq!(poly.smooth_type, Some(6));
    assert_eq!(poly.origin.map(|o| o[2]), Some(2.5));
    assert_eq!(poly.vertices.len(), 2);
}

#[test]
fn bulged_hatch_polyline_boundary_keeps_its_bulges() {
    let mut poly = acadrust::entities::hatch::PolylineEdge::new(Vec::new(), true);
    poly.add_vertex(Vector2::new(0.0, 0.0), 0.5);
    poly.add_vertex(Vector2::new(2.0, 0.0), 0.0);
    poly.add_vertex(Vector2::new(2.0, 2.0), 0.0);
    let mut hatch = acadrust::entities::Hatch::new();
    let mut path = acadrust::entities::hatch::BoundaryPath::new();
    path.add_edge(acadrust::entities::hatch::BoundaryEdge::Polyline(poly));
    hatch.add_path(path);
    let output = read_document(&common::document_with(
        DxfVersion::AC1032,
        EntityType::Hatch(Box::new(hatch)),
    ));
    let Some(hatch) = output.drawing.entities.first() else {
        panic!("the hatch is mapped");
    };
    let EntityKind::Hatch(hatch) = &hatch.kind else {
        panic!("a hatch is mapped to a hatch");
    };
    match &hatch.loops[0].boundary {
        HatchBoundary::Polyline {
            has_bulge,
            vertices,
            ..
        } => {
            assert!(*has_bulge);
            assert!((vertices[0].bulge - 0.5).abs() < 1e-12);
        }
        HatchBoundary::Edges(_) => panic!("a single polyline path keeps the polyline form"),
    }
}

#[test]
fn elliptic_hatch_edge_with_ratio_one_half_keeps_the_circular_angle() {
    let edge = acadrust::entities::hatch::EllipticArcEdge {
        center: Vector2::new(0.0, 0.0),
        major_axis_endpoint: Vector2::new(2.0, 0.0),
        minor_axis_ratio: 0.5,
        start_angle: std::f64::consts::FRAC_PI_4,
        end_angle: std::f64::consts::FRAC_PI_2,
        counter_clockwise: true,
    };
    let mut hatch = acadrust::entities::Hatch::new();
    let mut path = acadrust::entities::hatch::BoundaryPath::new();
    path.add_edge(acadrust::entities::hatch::BoundaryEdge::EllipticArc(edge));
    hatch.add_path(path);
    let output = read_document(&common::document_with(
        DxfVersion::AC1032,
        EntityType::Hatch(Box::new(hatch)),
    ));
    let Some(hatch) = output.drawing.entities.first() else {
        panic!("the hatch is mapped");
    };
    let EntityKind::Hatch(hatch) = &hatch.kind else {
        panic!("a hatch is mapped to a hatch");
    };
    let HatchBoundary::Edges(edges) = &hatch.loops[0].boundary else {
        panic!("an elliptic edge keeps the edge form");
    };
    let arc_io_dxf::model::HatchEdge::Ellipse {
        start_angle,
        end_angle,
        ..
    } = &edges[0]
    else {
        panic!("the edge is elliptic");
    };
    assert!(
        (start_angle - 0.5_f64.atan().to_degrees()).abs() < 1e-9,
        "{start_angle}"
    );
    assert!((end_angle - 90.0).abs() < 1e-9, "{end_angle}");
}

#[test]
fn mesh_records_are_read_without_reserving_faces() {
    let output = read_document(&common::document_with(
        DxfVersion::AC1032,
        EntityType::Mesh(acadrust::entities::Mesh::create_unit_cube()),
    ));
    let dropped =
        output.report.items.iter().find(|i| {
            i.reason == "io.dwg.entity_dropped" && i.record_kind.as_deref() == Some("MESH")
        });
    assert!(dropped.is_some(), "the mesh is reported as not carried");
}

/// The smallest `record_count` with which the bytes read, found by bisection; `None` when no limit
/// of the search range lets them read.
fn minimal_record_limit(bytes: &[u8]) -> Option<u64> {
    let reads = |record_count: u64| {
        let limits = Limits {
            record_count,
            ..Limits::CONSTRAINED
        };
        read(bytes, &mut ReadOptions::with_limits(limits)).is_ok()
    };
    let (mut low, mut high) = (1_u64, 1_u64 << 20);
    if !reads(high) {
        return None;
    }
    while low < high {
        let middle = low + (high - low) / 2;
        if reads(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Some(low)
}

#[test]
fn polyline_vertices_count_against_the_record_limit() {
    // The symbol tables already take most of a small limit, so the test compares the smallest limit
    // that reads the file with six vertices against the one with none: the vertices must add six.
    let polyline = |vertices: usize| {
        let mut poly = acadrust::entities::Polyline2D::new();
        for x in 0..vertices {
            let location = Vector3::new(f64::from(u32::try_from(x).unwrap_or(0)), 0.0, 0.0);
            poly.add_vertex(acadrust::entities::Vertex2D::new(location));
        }
        common::dwg_bytes(&common::document_with(
            DxfVersion::AC1032,
            EntityType::Polyline2D(poly),
        ))
    };
    let without = minimal_record_limit(&polyline(0));
    let with_six = minimal_record_limit(&polyline(6));
    assert!(without.is_some() && with_six.is_some());
    assert_eq!(with_six.zip(without).map(|(six, none)| six - none), Some(6));
}

#[test]
fn non_finite_ltscale_is_skipped_and_reported() {
    let mut doc = common::sample_document(DxfVersion::AC1032);
    doc.header.linetype_scale = f64::NAN;
    let output = read_document(&doc);
    assert!(!output.drawing.header.iter().any(|v| v.name == "$LTSCALE"));
    let skipped = output
        .report
        .items
        .iter()
        .find(|i| i.reason == "io.dwg.value_skipped");
    assert!(skipped.is_some());
}
