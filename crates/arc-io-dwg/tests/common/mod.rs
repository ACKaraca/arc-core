//! Builds the input of the tests: DWG bytes written by the wrapped library from a small document.

use acadrust::entities::AttributeEntity;
use acadrust::entities::hatch::{BoundaryEdge, BoundaryPath, PolylineEdge};
use acadrust::entities::{
    Arc as DwgArc, Circle as DwgCircle, EntityType, Hatch, Insert, Line as DwgLine, LwPolyline,
    MText, Solid, Text,
};
use acadrust::tables::{BlockRecord, Layer};
use acadrust::types::DxfVersion;
use acadrust::types::{Vector2, Vector3};
use acadrust::{CadDocument, DwgWriter};

/// A document of the given version with a layer, a line, an arc, a text, an lwpolyline, an
/// mtext, a hatch, an unmapped solid, and a block definition with a circle and an insert of it.
pub fn sample_document(version: DxfVersion) -> CadDocument {
    let mut doc = CadDocument::with_version(version);
    doc.layers.add_or_replace(Layer::new("WALLS"));
    doc.block_records.add_or_replace(BlockRecord::new("DOOR"));

    let mut line = DwgLine::from_points(Vector3::new(0.0, 0.0, 0.0), Vector3::new(10.0, 5.0, 0.0));
    line.common.layer = "WALLS".to_owned();
    add_model(&mut doc, EntityType::Line(line));

    let arc = DwgArc::from_center_radius_angles(
        Vector3::new(1.0, 2.0, 0.0),
        3.0,
        std::f64::consts::FRAC_PI_6,
        std::f64::consts::FRAC_PI_2,
    );
    add_model(&mut doc, EntityType::Arc(arc));

    let mut text = Text::with_value("ROOM", Vector3::new(4.0, 4.0, 0.0));
    text.height = 2.5;
    add_model(&mut doc, EntityType::Text(text));

    let mut poly = LwPolyline::new();
    poly.add_point(Vector2::new(0.0, 0.0));
    poly.add_point(Vector2::new(4.0, 0.0));
    poly.add_point(Vector2::new(4.0, 3.0));
    poly.is_closed = true;
    add_model(&mut doc, EntityType::LwPolyline(poly));

    let mtext = MText::with_value("NOTE", Vector3::new(6.0, 6.0, 0.0));
    add_model(&mut doc, EntityType::MText(Box::new(mtext)));

    let circle = EntityType::Circle(DwgCircle::from_center_radius(
        Vector3::new(0.0, 0.0, 0.0),
        1.0,
    ));
    let handle = doc.add_entity(circle).unwrap_or_default();
    if let Some(record) = doc.block_records.get_mut("DOOR") {
        record.entity_handles.push(handle);
    }

    let mut insert = Insert::new("DOOR", Vector3::new(20.0, 0.0, 0.0));
    insert
        .attributes
        .push(AttributeEntity::new("NUMBER".to_owned(), "101".to_owned()));
    add_model(&mut doc, EntityType::Insert(insert));

    let mut hatch = Hatch::new();
    let mut path = BoundaryPath::new();
    path.add_edge(BoundaryEdge::Polyline(PolylineEdge::new(
        vec![
            Vector2::new(0.0, 0.0),
            Vector2::new(2.0, 0.0),
            Vector2::new(2.0, 2.0),
        ],
        true,
    )));
    hatch.add_path(path);
    add_model(&mut doc, EntityType::Hatch(Box::new(hatch)));

    let solid = Solid::new(
        Vector3::new(0.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, 1.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    );
    add_model(&mut doc, EntityType::Solid(solid));
    doc
}

fn add_model(doc: &mut CadDocument, entity: EntityType) {
    let handle = doc.add_entity(entity).unwrap_or_default();
    if let Some(record) = doc.block_records.get_mut("*Model_Space") {
        record.entity_handles.push(handle);
    }
}

/// The DWG bytes of a document.
pub fn dwg_bytes(doc: &CadDocument) -> Vec<u8> {
    DwgWriter::write_to_vec(doc).unwrap_or_default()
}
