//! A DWG fixture built in memory and written by the wrapped library, for tests of this crate and
//! of the products that read DWG files. Enabled by the `testing` feature, which is off by default
//! and not part of the WebAssembly build; the crate's own tests always include it.

use acadrust::entities::{Circle, EntityType, Insert, Line};
use acadrust::tables::{BlockRecord, Layer};
use acadrust::types::{DxfVersion, Vector3};
use acadrust::{CadDocument, DwgWriter};

/// The fingerprint identifier written into the fixture's header.
pub const FINGERPRINT_GUID: &str = "{5F1C0A2E-7B3D-4C8A-9E61-2D4F0B7A9C13}";
/// The version identifier written into the fixture's header.
pub const VERSION_GUID: &str = "{A0B1C2D3-E4F5-4A6B-8C7D-9E0F1A2B3C4D}";

/// Builds a small R2018 drawing in memory and returns the bytes of its DWG file.
///
/// The drawing has the layer `WALLS`, a line on it in model space, a block `DOOR` whose
/// definition holds a circle of radius 1 and whose record units are inches, and one insert of
/// `DOOR`. The header holds [`FINGERPRINT_GUID`], [`VERSION_GUID`] and millimeters as the insertion
/// units.
///
/// # Errors
///
/// An [`std::io::Error`] when the library refuses to add an entity or to write the document. The
/// fixture is fixed, so this does not happen unless the library changes its behavior.
pub fn sample_dwg_bytes() -> std::io::Result<Vec<u8>> {
    let mut doc = CadDocument::with_version(DxfVersion::AC1032);
    doc.header.fingerprint_guid = FINGERPRINT_GUID.to_owned();
    doc.header.version_guid = VERSION_GUID.to_owned();
    doc.header.insertion_units = 4;
    doc.header.current_layer_name = "WALLS".to_owned();
    doc.layers.add_or_replace(Layer::new("WALLS"));
    let mut door = BlockRecord::new("DOOR");
    door.units = 1;
    doc.block_records.add_or_replace(door);

    let mut line = Line::from_points(Vector3::new(0.0, 0.0, 0.0), Vector3::new(10.0, 5.0, 0.0));
    line.common.layer = "WALLS".to_owned();
    add_model(&mut doc, EntityType::Line(line))?;

    let circle = doc.add_entity(EntityType::Circle(Circle::from_center_radius(
        Vector3::new(0.0, 0.0, 0.0),
        1.0,
    )));
    let circle = library(circle)?;
    if let Some(record) = doc.block_records.get_mut("DOOR") {
        record.entity_handles.push(circle);
    }

    add_model(
        &mut doc,
        EntityType::Insert(Insert::new("DOOR", Vector3::new(20.0, 0.0, 0.0))),
    )?;

    library(DwgWriter::write_to_vec(&doc))
}

/// Adds `entity` to the model space record of `doc`.
fn add_model(doc: &mut CadDocument, entity: EntityType) -> std::io::Result<()> {
    let handle = library(doc.add_entity(entity))?;
    if let Some(record) = doc.block_records.get_mut("*Model_Space") {
        record.entity_handles.push(handle);
    }
    Ok(())
}

/// A library error as an I/O error of this module; the library's error type is not exposed.
fn library<T, E: std::fmt::Debug>(result: Result<T, E>) -> std::io::Result<T> {
    result.map_err(|error| std::io::Error::other(format!("{error:?}")))
}

#[cfg(test)]
mod tests {
    use arc_base::Limits;
    use arc_io_dxf::Group;
    use arc_io_dxf::model::EntityKind;

    use super::*;
    use crate::read;
    use arc_io_dxf::ReadOptions;

    #[test]
    fn the_fixture_reads_back_with_its_entities_layers_block_and_identity() {
        let bytes = match sample_dwg_bytes() {
            Ok(bytes) => bytes,
            Err(err) => panic!("the fixture is written: {err}"),
        };
        let mut options = ReadOptions::with_limits(Limits::CONSTRAINED);
        let output = match read(&bytes, &mut options) {
            Ok(output) => output,
            Err(err) => panic!("the fixture is read: {}", err.code()),
        };
        let drawing = &output.drawing;

        let top: Vec<&str> = drawing
            .entities
            .iter()
            .map(|e| e.kind.type_name())
            .collect();
        assert_eq!(top, ["LINE", "INSERT"]);
        assert_eq!(drawing.entities[0].props.layer, "WALLS");

        let layers = drawing.tables.layers.as_ref().map(|t| {
            t.entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
        });
        assert!(layers.is_some_and(|names| names.contains(&"WALLS")));

        let door = drawing
            .blocks
            .iter()
            .find(|b| matches!(&b.begin.kind, EntityKind::Block(begin) if begin.name == "DOOR"));
        let circle = door.and_then(|b| {
            b.entities.iter().find_map(|e| match &e.kind {
                EntityKind::Circle(c) => Some(c.radius),
                _ => None,
            })
        });
        assert_eq!(circle, Some(1.0));
        let units = drawing
            .tables
            .block_records
            .as_ref()
            .and_then(|t| t.entries.iter().find(|e| e.name == "DOOR"))
            .map(|e| e.flags);
        assert_eq!(units, Some(1), "the block record keeps its insertion units");

        let text = |name: &str| {
            drawing
                .header_variable(name)
                .and_then(|v| v.first())
                .and_then(Group::as_text)
                .map(str::to_owned)
        };
        assert_eq!(text("$FINGERPRINTGUID").as_deref(), Some(FINGERPRINT_GUID));
        assert_eq!(text("$VERSIONGUID").as_deref(), Some(VERSION_GUID));
        assert!(
            text("$HANDSEED").is_some_and(|seed| !seed.is_empty()),
            "the handle seed is kept"
        );
    }
}
