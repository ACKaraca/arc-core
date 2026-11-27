//! The writer in the three output schemas, self-check, IFC-ZIP, model views and spatial elements.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

mod support;

use arc_base::math::{Point2, Point3, Vector3};
use arc_io_ifc::write::Georeference;
use arc_io_ifc::write::{
    Actor, Address, Body, Container, Element, ElementKind, ElementType, Group, MaterialLayer,
    ModelView, OutputSchema, Party, Property, PropertySet, PropertyValue, Quantity, QuantitySet,
    QuantityValue, SelfCheck, Spatial, Storey, ViewCheck,
};
use arc_io_ifc::{ReadOptions, WriteModel, WriteOptions, read, write};

const STOREY: u128 = 0x100;
const WALL: u128 = 0x200;
const WALL_TYPE: u128 = 0x201;
const OPENING: u128 = 0x300;
const DOOR: u128 = 0x301;
const DOOR_TYPE: u128 = 0x302;
const WINDOW_TYPE: u128 = 0x303;
const SLAB: u128 = 0x400;
const ROOF: u128 = 0x500;
const ROOF_TYPE: u128 = 0x501;
const STAIR: u128 = 0x600;
const FLIGHT: u128 = 0x601;
const SPACE: u128 = 0x700;
const PIPE: u128 = 0x800;
const GROUP: u128 = 0x900;

fn rectangle(x: f64, y: f64) -> Vec<Point2> {
    vec![
        Point2::new(0.0, 0.0),
        Point2::new(x, 0.0),
        Point2::new(x, y),
        Point2::new(0.0, y),
    ]
}

fn extrusion(x: f64, y: f64, depth: f64) -> Option<Body> {
    Some(Body::Extrusion {
        outer: rectangle(x, y),
        holes: Vec::new(),
        direction: Vector3::z(),
        depth,
    })
}

fn brep() -> Option<Body> {
    let v = |x, y, z| Point3::new(x, y, z);
    Some(Body::Brep {
        vertices: vec![
            v(0.0, 0.0, 0.0),
            v(2.0, 0.0, 0.0),
            v(2.0, 2.0, 0.0),
            v(0.0, 2.0, 0.0),
            v(0.0, 0.0, 1.0),
            v(2.0, 0.0, 1.0),
            v(2.0, 2.0, 1.0),
            v(0.0, 2.0, 1.0),
        ],
        faces: vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ],
    })
}

fn model() -> WriteModel {
    let mut project = Spatial::new(1, "Project");
    project.long_name = Some("The project".into());
    project.description = Some("Description".into());
    project.phase = Some("Design".into());
    project.actors = vec![{
        let mut a = Actor::new(0x1000, "Architect", Party::Organization("Studio".into()));
        a.role = Some("architect".into());
        a
    }];
    let mut site = Spatial::new(2, "Site");
    site.long_name = Some("Plot 7".into());
    let mut address = Address::default();
    address.lines = vec!["1 Main Street".into()];
    address.town = Some("Town".into());
    address.country = Some("TR".into());
    address.purpose = Some("site".into());
    site.address = Some(address);
    site.property_sets = vec![PropertySet::new(
        "Pset_SiteCommon",
        vec![Property::new("TotalArea", PropertyValue::Area(1200.0))],
    )];
    let mut building = Spatial::new(3, "Building");
    building.actors = vec![{
        let mut a = Actor::new(
            0x1001,
            "Owner",
            Party::Person {
                family_name: Some("Doe".into()),
                given_name: Some("Jane".into()),
            },
        );
        a.role = Some("CLIENT".into());
        a
    }];
    building.property_sets = vec![PropertySet::new(
        "Pset_BuildingCommon",
        vec![Property::new("NumberOfStoreys", PropertyValue::Integer(1))],
    )];
    let mut model = WriteModel::new(project, site, building);
    let mut storey = Storey::new(STOREY, "Ground", 0.0);
    storey.long_name = Some("Ground floor".into());
    model.storeys.push(storey);

    let mut wall_type = ElementType::new(WALL_TYPE, ElementKind::Wall, "Cavity");
    wall_type.predefined_type = Some("STANDARD".into());
    wall_type.layers = vec![
        MaterialLayer::new("Brick", 0.1),
        MaterialLayer::new("Air", 0.05),
    ];
    let mut door_type = ElementType::new(DOOR_TYPE, ElementKind::Door, "D1");
    door_type.predefined_type = Some("DOOR".into());
    door_type.operation_type = Some("SINGLE_SWING_LEFT".into());
    let mut window_type = ElementType::new(WINDOW_TYPE, ElementKind::Window, "W1");
    window_type.operation_type = Some("SINGLE_PANEL".into());
    let mut roof_type = ElementType::new(ROOF_TYPE, ElementKind::Roof, "R1");
    roof_type.predefined_type = Some("HIP_ROOF".into());
    model.types = vec![wall_type, door_type, window_type, roof_type];

    let mut wall = Element::new(WALL, ElementKind::Wall, "Wall", STOREY);
    wall.body = extrusion(5.0, 0.2, 3.0);
    wall.predefined_type = Some("STANDARD".into());
    wall.element_type = Some(WALL_TYPE);
    wall.property_sets = vec![PropertySet::new(
        "Pset_WallCommon",
        vec![
            Property::new("IsExternal", PropertyValue::Boolean(true)),
            Property::new(
                "ThermalTransmittance",
                PropertyValue::measure(arc_io_ifc::write::Measure::ThermalTransmittance, 0.3)
                    .unwrap(),
            ),
        ],
    )];
    wall.quantity_sets = vec![QuantitySet::new(
        "Qto_WallBaseQuantities",
        vec![
            Quantity::new("Length", QuantityValue::Length(5.0)),
            Quantity::new("NetVolume", QuantityValue::Volume(3.0)),
        ],
    )];
    let mut opening = Element::new(OPENING, ElementKind::Opening, "Opening", STOREY);
    opening.body = extrusion(1.0, 0.3, 2.1);
    opening.host = Some(WALL);
    let mut door = Element::new(DOOR, ElementKind::Door, "Door", STOREY);
    door.body = extrusion(0.9, 0.1, 2.1);
    door.host = Some(OPENING);
    door.predefined_type = Some("DOOR".into());
    door.operation_type = Some("SINGLE_SWING_LEFT".into());
    door.overall_width = Some(0.9);
    door.overall_height = Some(2.1);
    door.element_type = Some(DOOR_TYPE);
    let mut slab = Element::new(SLAB, ElementKind::Slab, "Slab", STOREY);
    slab.body = extrusion(6.0, 4.0, 0.2);
    slab.predefined_type = Some("FLOOR".into());
    let mut roof = Element::new(ROOF, ElementKind::Roof, "Roof", STOREY);
    roof.body = brep();
    roof.predefined_type = Some("HIP_ROOF".into());
    roof.element_type = Some(ROOF_TYPE);
    let mut stair = Element::new(STAIR, ElementKind::Stair, "Stair", STOREY);
    stair.predefined_type = Some("STRAIGHT_RUN_STAIR".into());
    let mut flight = Element::new(FLIGHT, ElementKind::StairFlight, "Flight", STOREY);
    flight.host = Some(STAIR);
    flight.body = extrusion(1.0, 3.0, 0.3);
    flight.flight = Some(arc_io_ifc::write::FlightDimensions::new(10, 9, 0.17, 0.28));
    let mut space = Element::new(SPACE, ElementKind::Space, "101", STOREY);
    space.long_name = Some("Room".into());
    space.predefined_type = Some("SPACE".into());
    space.body = extrusion(4.0, 3.0, 2.8);
    let mut pipe = Element::new(PIPE, ElementKind::PipeSegment, "Pipe", STOREY);
    pipe.body = extrusion(0.1, 0.1, 2.0);
    model.elements = vec![wall, opening, door, slab, roof, stair, flight, space, pipe];
    model.groups = vec![Group::new(GROUP, "Group", vec![WALL, PIPE])];
    model.georeference = Some(Georeference::new(
        "EPSG:32635",
        500_000.0,
        4_000_000.0,
        10.0,
    ));
    model
}

fn options(schema: OutputSchema) -> WriteOptions {
    WriteOptions {
        file_name: "model.ifc".into(),
        timestamp: "2026-01-02T03:04:05".into(),
        author: "Author".into(),
        organization: "Organization".into(),
        originating_system: "test".into(),
        schema,
        ..WriteOptions::default()
    }
}

#[cfg(arc_ifc_tables_ifc2x3)]
fn codes(output: &arc_io_ifc::write::WriteOutput) -> Vec<(String, String)> {
    output
        .report
        .items
        .iter()
        .map(|i| {
            (
                i.reason.to_string(),
                i.record_kind.clone().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn ifc4x3_is_written_through_the_tables_without_findings() {
    let output = write(&model(), &options(OutputSchema::Ifc4x3)).unwrap();
    support::dump("schemas-ifc4x3.ifc", &output.bytes);
    let text = String::from_utf8(output.bytes.clone()).unwrap();
    assert!(text.contains("FILE_SCHEMA(('IFC4X3_ADD2'))"));
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    assert_eq!(file.schema, arc_io_ifc::Schema::Ifc4x3);
    let found = file
        .conformance(&arc_base::Limits::DEFAULT, &arc_base::Cancel::new())
        .unwrap();
    assert!(
        found.schema.is_some() && found.findings.is_empty(),
        "{:?}",
        found.findings
    );
    assert_eq!(found.failed(), 0);
    assert!(output.findings.is_empty());
    assert_eq!(output.report.header.version, "IFC4X3_ADD2");
    // Description of a property is Specification: no loss, same count of attributes.
    assert!(text.contains("IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.T.),$)"));
    assert!(text.contains("IFCPOSTALADDRESS(.SITE.,$,$,$,('1 Main Street'),$,'Town',$,$,'TR')"));
    assert!(text.contains("IFCACTORROLE(.ARCHITECT.,$,$)"));
}

#[cfg(arc_ifc_tables_ifc2x3)]
#[test]
fn ifc2x3_maps_and_reports_what_it_cannot_carry() {
    let output = write(&model(), &options(OutputSchema::Ifc2x3)).unwrap();
    support::dump("schemas-ifc2x3.ifc", &output.bytes);
    let text = String::from_utf8(output.bytes.clone()).unwrap();
    assert!(text.contains("FILE_SCHEMA(('IFC2X3'))"));
    assert!(text.contains("IFCOWNERHISTORY("));
    assert!(text.contains("IFCDOORSTYLE("));
    assert!(text.contains("IFCWINDOWSTYLE("));
    assert!(!text.contains("IFCROOFTYPE"));
    assert!(!text.contains("IFCMAPCONVERSION"));
    assert!(!text.contains("IFCPIPESEGMENT("));
    assert!(text.contains("'BaseQuantities'"));
    let reported = codes(&output);
    for expected in [
        ("io.ifc.write_entity_unrepresentable", "IFCROOFTYPE"),
        ("io.ifc.write_entity_unrepresentable", "IFCPIPESEGMENT"),
        ("io.ifc.write_entity_unrepresentable", "IFCMAPCONVERSION"),
        ("io.ifc.write_set_renamed", "IFCELEMENTQUANTITY"),
        (
            "io.ifc.write_attribute_unrepresentable",
            "IFCWALL.PredefinedType",
        ),
    ] {
        assert!(
            reported.contains(&(expected.0.to_owned(), expected.1.to_owned())),
            "{expected:?} in {reported:?}"
        );
    }
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    let found = file
        .conformance(&arc_base::Limits::DEFAULT, &arc_base::Cancel::new())
        .unwrap();
    assert!(found.findings.is_empty(), "{:?}", found.findings);
}

#[cfg(arc_ifc_tables_ifc4)]
#[test]
fn ifc4_passes_the_self_check_and_its_bytes_do_not_change() {
    let output = write(&model(), &options(OutputSchema::Ifc4)).unwrap();
    support::dump("schemas-ifc4.ifc", &output.bytes);
    assert!(output.findings.is_empty());
}

#[cfg(not(arc_ifc_tables_ifc2x3))]
#[test]
fn ifc2x3_without_tables_is_a_typed_error() {
    let error = write(&model(), &options(OutputSchema::Ifc2x3)).unwrap_err();
    assert_eq!(error.code(), "io.ifc.write_schema_unavailable");
}

#[test]
fn zip_output_reads_back() {
    let mut o = options(OutputSchema::Ifc4x3);
    let plain = write(&model(), &o).unwrap();
    o.container = Container::Zip;
    let zipped = write(&model(), &o).unwrap();
    assert_eq!(&zipped.bytes[..2], b"PK");
    let file = read(&zipped.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    let again = read(&plain.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    assert_eq!(file.instances().len(), again.instances().len());
}

#[test]
fn spatial_elements_carry_names_addresses_actors_and_sets() {
    let output = write(&model(), &options(OutputSchema::Ifc4x3)).unwrap();
    let text = String::from_utf8(output.bytes).unwrap();
    assert!(text.contains("'The project'"));
    assert!(text.contains("'Design'"));
    assert!(text.contains("'Plot 7'"));
    assert!(text.contains("'Ground floor'"));
    assert!(text.contains("IFCORGANIZATION($,'Studio',$,$,$)"));
    assert!(text.contains("IFCPERSON($,'Doe','Jane',$,$,$,$,$)"));
    assert_eq!(text.matches("IFCRELASSIGNSTOACTOR(").count(), 2);
    assert!(text.contains("Pset_SiteCommon") && text.contains("Pset_BuildingCommon"));
    let mut bad = model();
    bad.site.address = Some(Address::default());
    let error = write(&bad, &options(OutputSchema::Ifc4)).unwrap_err();
    assert_eq!(error.code(), "io.ifc.write_value_invalid");
}

#[test]
fn model_views_are_checked_and_named_only_when_satisfied() {
    let mut o = options(OutputSchema::Ifc4x3);
    o.view_check = ViewCheck::Report(ModelView::DesignTransferView);
    let output = write(&model(), &o).unwrap();
    assert!(
        output.view_findings.is_empty(),
        "{:?}",
        output.view_findings
    );
    assert!(
        String::from_utf8(output.bytes)
            .unwrap()
            .contains("ViewDefinition [DesignTransferView_V1.0]")
    );
    // Extrusions and B-reps are not tessellated: the Reference View is not satisfied.
    o.view_check = ViewCheck::Report(ModelView::ReferenceView);
    let output = write(&model(), &o).unwrap();
    assert!(!output.view_findings.is_empty());
    assert!(
        output
            .view_findings
            .iter()
            .all(|f| f.code == "io.ifc.view_representation_not_allowed")
    );
    assert!(
        String::from_utf8(output.bytes)
            .unwrap()
            .contains("ViewDefinition []")
    );
    o.view_check = ViewCheck::Enforce(ModelView::ReferenceView);
    assert_eq!(
        write(&model(), &o).unwrap_err().code(),
        "io.ifc.write_view_violated"
    );
}

#[test]
fn self_check_off_skips_the_read_back() {
    let mut o = options(OutputSchema::Ifc4x3);
    o.self_check = SelfCheck::Off;
    assert!(write(&model(), &o).is_ok());
}
