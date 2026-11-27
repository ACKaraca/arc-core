//! IFC writing (CORE-IO-016): encoding, determinism, limits, and a read-back of a small house by
//! the crate's own reader, typed views and geometry evaluation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

use std::collections::BTreeMap;

mod support;

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Cancel, Limits};
use arc_io_ifc::write::{
    AngleUnit, Body, Element, ElementKind, ElementType, LayerDirection, LayerSetUsage, LengthUnit,
    MaterialLayer, Placement, Property, PropertySet, PropertyValue, Quantity, QuantitySet,
    QuantityValue, Spatial, Storey,
};
use arc_io_ifc::{
    IfcFile, Parameter, ReadOptions, SpatialNode, WriteModel, WriteOptions, global_id, read,
    reasons, write,
};
use arc_mesh::measure::volume;

const GROUND: u128 = 0x100;
const UPPER: u128 = 0x101;
const WALLS: [u128; 4] = [0x200, 0x201, 0x202, 0x203];
const SLAB: u128 = 0x300;
const DOOR_OPENING: u128 = 0x400;
const WINDOW_OPENING: u128 = 0x401;
const DOOR: u128 = 0x500;
const WINDOW: u128 = 0x600;

fn rectangle(x: f64, y: f64) -> Vec<Point2> {
    vec![
        Point2::new(0.0, 0.0),
        Point2::new(x, 0.0),
        Point2::new(x, y),
        Point2::new(0.0, y),
    ]
}

fn extrusion(x: f64, y: f64, depth: f64) -> Body {
    Body::Extrusion {
        outer: rectangle(x, y),
        holes: Vec::new(),
        direction: Vector3::z(),
        depth,
    }
}

/// A box from the origin to `(x, y, z)` as a faceted B-rep, faces counterclockwise from outside.
fn box_brep(x: f64, y: f64, z: f64) -> Body {
    let vertices = (0..8)
        .map(|v| {
            Point3::new(
                f64::from(v & 1) * x,
                f64::from((v >> 1) & 1) * y,
                f64::from((v >> 2) & 1) * z,
            )
        })
        .collect();
    let faces = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ]
    .iter()
    .map(|f| f.to_vec())
    .collect();
    Body::Brep { vertices, faces }
}

fn placed(x: f64, y: f64, z: f64, ref_direction: Vector3) -> Placement {
    Placement::new(Point3::new(x, y, z), Vector3::z(), ref_direction)
}

/// Four walls (three extruded, one B-rep), a slab with a hole on the upper storey, and a door and
/// a window, each filling an opening in a wall.
fn house() -> WriteModel {
    let wall = |id, name: &str, placement, body| {
        let mut wall = Element::new(id, ElementKind::Wall, name, GROUND);
        wall.placement = placement;
        wall.body = Some(body);
        wall.predefined_type = Some("standard".into());
        wall
    };
    let mut first = wall(
        WALLS[0],
        "Wall 'south' ğ",
        Placement::default(),
        extrusion(10.0, 0.2, 3.0),
    );
    first.property_sets.push(PropertySet::new(
        "Pset_WallCommon",
        vec![
            Property::new("IsExternal", PropertyValue::Boolean(true)),
            Property::new("Reference", PropertyValue::Label("W-01 \\ ç".into())),
            Property::new("Width", PropertyValue::Length(0.2)),
        ],
    ));
    first.quantity_sets.push(QuantitySet::new(
        "Qto_WallBaseQuantities",
        vec![
            Quantity::new("Length", QuantityValue::Length(10.0)),
            Quantity::new("GrossSideArea", QuantityValue::Area(30.0)),
            Quantity::new("GrossVolume", QuantityValue::Volume(6.0)),
        ],
    ));
    let mut slab = Element::new(SLAB, ElementKind::Slab, "Floor", UPPER);
    slab.predefined_type = Some("FLOOR".into());
    slab.body = Some(Body::Extrusion {
        outer: rectangle(10.0, 8.0),
        holes: vec![vec![
            Point2::new(4.0, 3.0),
            Point2::new(4.0, 4.0),
            Point2::new(5.0, 4.0),
            Point2::new(5.0, 3.0),
            Point2::new(4.0, 3.0),
        ]],
        direction: Vector3::z(),
        depth: 0.2,
    });
    let mut door_opening = Element::new(DOOR_OPENING, ElementKind::Opening, "O1", GROUND);
    door_opening.placement = placed(2.0, 0.0, 0.0, Vector3::x());
    door_opening.body = Some(extrusion(0.9, 0.2, 2.1));
    door_opening.host = Some(WALLS[0]);
    let mut window_opening = Element::new(WINDOW_OPENING, ElementKind::Opening, "O2", GROUND);
    window_opening.placement = placed(6.0, 8.0, 0.9, -Vector3::x());
    window_opening.body = Some(extrusion(1.2, 0.2, 1.2));
    window_opening.host = Some(WALLS[2]);
    let mut door = Element::new(DOOR, ElementKind::Door, "Door", GROUND);
    door.placement = placed(2.0, 0.075, 0.0, Vector3::x());
    door.body = Some(box_brep(0.9, 0.05, 2.1));
    door.host = Some(DOOR_OPENING);
    door.predefined_type = Some("DOOR".into());
    door.operation_type = Some("SINGLE_SWING_LEFT".into());
    door.overall_width = Some(0.9);
    door.overall_height = Some(2.1);
    let mut window = Element::new(WINDOW, ElementKind::Window, "Window", GROUND);
    window.placement = placed(6.0, 7.925, 0.9, -Vector3::x());
    window.body = Some(extrusion(1.2, 0.05, 1.2));
    window.host = Some(WINDOW_OPENING);
    window.predefined_type = Some("WINDOW".into());
    window.operation_type = Some("tilt and turn".into());
    window.overall_width = Some(1.2);
    window.overall_height = Some(1.2);
    let mut model = WriteModel::new(
        Spatial::new(0x10, "House"),
        Spatial::new(0x11, "Site"),
        Spatial::new(0x12, "Building"),
    );
    model.storeys = vec![
        Storey::new(GROUND, "Ground", 0.0),
        Storey::new(UPPER, "Upper", 3.0),
    ];
    model.elements = vec![
        first,
        wall(
            WALLS[1],
            "Wall east",
            placed(10.0, 0.0, 0.0, Vector3::y()),
            extrusion(8.0, 0.2, 3.0),
        ),
        wall(
            WALLS[2],
            "Wall north",
            placed(10.0, 8.0, 0.0, -Vector3::x()),
            extrusion(10.0, 0.2, 3.0),
        ),
        wall(
            WALLS[3],
            "Wall west",
            placed(0.0, 8.0, 0.0, -Vector3::y()),
            box_brep(8.0, 0.2, 3.0),
        ),
        slab,
        door_opening,
        window_opening,
        door,
        window,
    ];
    model.angle_unit = AngleUnit::Degree;
    model.true_north = Some(Vector2::new(0.0, 1.0));
    model
}

fn options() -> WriteOptions {
    WriteOptions {
        file_name: "house.ifc".into(),
        timestamp: "2026-01-02T03:04:05".into(),
        author: "Author".into(),
        organization: "Organization".into(),
        originating_system: "test".into(),
        ..WriteOptions::default()
    }
}

fn read_back(bytes: &[u8]) -> IfcFile {
    read(bytes, &mut ReadOptions::default()).expect("read").file
}

/// The `GlobalId` text of an instance.
fn gid_of(file: &IfcFile, name: u64) -> String {
    match file.instance(name).unwrap().records[0].parameters.first() {
        Some(Parameter::Text(text)) => text.clone(),
        other => panic!("no GlobalId: {other:?}"),
    }
}

/// The instance name of the object with the `GlobalId` of `id`.
fn instance_of(file: &IfcFile, id: u128) -> u64 {
    let wanted = global_id(id);
    file.instances()
        .iter()
        .find(
            |i| matches!(i.records[0].parameters.first(), Some(Parameter::Text(t)) if *t == wanted),
        )
        .map(|i| i.name)
        .expect("instance")
}

/// The volume in cubic meters of every product with geometry, by `GlobalId`.
fn volumes(file: &IfcFile) -> (BTreeMap<String, f64>, arc_io_ifc::LossReport) {
    let geometry = file.geometry(&Limits::CONSTRAINED, &Cancel::new()).unwrap();
    let mut out = BTreeMap::new();
    for item in &geometry.items {
        let mesh = item.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap();
        *out.entry(gid_of(file, item.product)).or_insert(0.0) +=
            volume(&mesh, &Limits::CONSTRAINED).unwrap();
    }
    (out, geometry.report)
}

#[test]
fn output_is_deterministic_and_has_the_header() {
    let first = write(&house(), &options()).unwrap();
    let second = write(&house(), &options()).unwrap();
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.report, second.report);
    let text = String::from_utf8(first.bytes).unwrap();
    assert!(text.starts_with(
        "ISO-10303-21;\nHEADER;\n\
         FILE_DESCRIPTION(('ViewDefinition [DesignTransferView_V1.0]'),'2;1');\n\
         FILE_NAME('house.ifc','2026-01-02T03:04:05',('Author'),('Organization'),'arc-io-ifc "
    ));
    assert!(text.contains("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1="));
    assert!(text.ends_with("ENDSEC;\nEND-ISO-10303-21;\n"));
    assert!(text.contains("'Wall ''south'' \\X2\\011F\\X0\\'"));
    assert!(text.contains("IFCLABEL('W-01 \\\\ \\X2\\00E7\\X0\\')"));
    assert!(text.contains("IFCBOOLEAN(.T.)"));
    assert!(text.contains(".DOOR.,.SINGLE_SWING_LEFT.,$);"));
    assert!(text.contains(".WINDOW.,.USERDEFINED.,'tilt and turn');"));
    assert!(text.contains(",.STANDARD.);"));
    assert_eq!(first.report.summary.mapped, 9);
    assert!(!first.report.has_loss());
}

/// Renaming one wall of a written file changes its line only; the oracle validates the result.
#[test]
fn an_edited_house_changes_one_line() {
    let output = write(&house(), &options()).unwrap();
    let file = read_back(&output.bytes);
    let mut edited = file.clone();
    let wall = edited.instances_of("IFCWALL").next().unwrap().name;
    edited
        .set_attribute(wall, 0, 2, Parameter::Text("Wall renamed \u{e7}".into()))
        .unwrap();
    let after = edited.write_step(&Limits::CONSTRAINED).unwrap();
    support::dump("house-edited.ifc", &after);
    let before = String::from_utf8(output.bytes.clone()).unwrap();
    let after = String::from_utf8(after).unwrap();
    let changed: Vec<_> = before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(before.lines().count(), after.lines().count());
    assert_eq!(changed.len(), 1);
    assert!(changed[0].1.contains("'Wall renamed \\X2\\00E7\\X0\\'"));
}

#[test]
fn house_reads_back_with_structure_relations_and_volumes() {
    let output = write(&house(), &options()).unwrap();
    support::dump("house.ifc", &output.bytes);
    let file = read_back(&output.bytes);

    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    assert!(!views.report.has_loss(), "{:?}", views.report);

    // Spatial structure: project, site, building, two storeys with their elements.
    assert_eq!(views.spatial.len(), 1);
    let project = &views.spatial[0];
    assert_eq!(project.entity.global_id.as_deref(), Some(&*global_id(0x10)));
    assert_eq!(project.entity.name.as_deref(), Some("House"));
    let site = &project.children[0];
    let building = &site.children[0];
    assert_eq!(site.entity.type_name, "IFCSITE");
    assert_eq!(building.entity.type_name, "IFCBUILDING");
    let storeys: Vec<&SpatialNode> = building.children.iter().collect();
    assert_eq!(storeys.len(), 2);
    let ids = |node: &SpatialNode| -> Vec<String> {
        node.contained
            .iter()
            .map(|e| e.global_id.clone().unwrap())
            .collect()
    };
    let ground: Vec<String> = [WALLS[0], WALLS[1], WALLS[2], WALLS[3], DOOR, WINDOW]
        .map(global_id)
        .to_vec();
    assert_eq!(ids(storeys[0]), ground);
    assert_eq!(ids(storeys[1]), vec![global_id(SLAB)]);
    assert_eq!(
        storeys[0].contained[0].name.as_deref(),
        Some("Wall 'south' ğ")
    );

    // Voids and fills.
    let pairs = |type_name: &str| -> Vec<(String, String)> {
        file.instances_of(type_name)
            .map(|i| match &i.records[0].parameters[4..] {
                [Parameter::Reference(a), Parameter::Reference(b)] => {
                    (gid_of(&file, *a), gid_of(&file, *b))
                }
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(
        pairs("IFCRELVOIDSELEMENT"),
        vec![
            (global_id(WALLS[0]), global_id(DOOR_OPENING)),
            (global_id(WALLS[2]), global_id(WINDOW_OPENING)),
        ]
    );
    assert_eq!(
        pairs("IFCRELFILLSELEMENT"),
        vec![
            (global_id(DOOR_OPENING), global_id(DOOR)),
            (global_id(WINDOW_OPENING), global_id(WINDOW)),
        ]
    );

    // Door sizes are at OverallHeight (8) and OverallWidth (9).
    let door = file.instance(instance_of(&file, DOOR)).unwrap();
    assert_eq!(door.records[0].type_name, "IFCDOOR");
    assert_eq!(door.records[0].parameters[8], Parameter::Real(2.1));
    assert_eq!(door.records[0].parameters[9], Parameter::Real(0.9));

    // Property and quantity sets of the first wall.
    assert_eq!(views.property_sets.len(), 1);
    let set = &views.property_sets[0];
    assert_eq!(set.definition.name.as_deref(), Some("Pset_WallCommon"));
    assert_eq!(set.related[0].global_id, Some(global_id(WALLS[0])));
    let names: Vec<_> = set
        .properties
        .iter()
        .map(|p| p.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["IsExternal", "Reference", "Width"]);
    assert!(matches!(
        &set.properties[1].value,
        arc_io_ifc::PropertyValue::Single { nominal: Some(n), .. }
            if n.value == arc_io_ifc::Value::Text("W-01 \\ ç".into())
    ));
    assert_eq!(views.quantity_sets.len(), 1);
    let quantities: Vec<_> = views.quantity_sets[0]
        .quantities
        .iter()
        .map(|q| (q.kind, q.value))
        .collect();
    assert_eq!(
        quantities,
        [
            (
                arc_io_ifc::QuantityKind::Length,
                Some(arc_io_ifc::Number::Real(10.0))
            ),
            (
                arc_io_ifc::QuantityKind::Area,
                Some(arc_io_ifc::Number::Real(30.0))
            ),
            (
                arc_io_ifc::QuantityKind::Volume,
                Some(arc_io_ifc::Number::Real(6.0))
            ),
        ]
    );
}

/// The reader does not subtract openings, so each body keeps its own volume.
#[test]
fn house_body_volumes_read_back() {
    let file = read_back(&write(&house(), &options()).unwrap().bytes);
    let (volumes, report) = volumes(&file);
    let expected = [
        (WALLS[0], 6.0),
        (WALLS[1], 4.8),
        (WALLS[2], 6.0),
        (WALLS[3], 4.8),
        (SLAB, 15.8),
        (DOOR_OPENING, 0.378),
        (WINDOW_OPENING, 0.288),
        (DOOR, 0.0945),
        (WINDOW, 0.072),
    ];
    assert_eq!(volumes.len(), expected.len());
    for (id, value) in expected {
        let got = volumes[&global_id(id)];
        assert!((got - value).abs() < 1e-9, "{id:x}: {got} != {value}");
    }
    let unsubtracted = report
        .items
        .iter()
        .find(|i| i.reason == reasons::OPENING_NOT_SUBTRACTED.code)
        .unwrap();
    assert_eq!(
        unsubtracted.arguments.get("count"),
        Some(&arc_io_ifc::loss::Argument::Int(2))
    );
    assert_eq!(report.summary.dropped, 0);
}

#[test]
fn millimetres_read_back_in_meters() {
    let mut model = house();
    model.length_unit = LengthUnit::Millimetre;
    model.angle_unit = AngleUnit::Radian;
    model.storeys[1].elevation = 3000.0;
    model.elements = vec![{
        let mut wall = Element::new(WALLS[0], ElementKind::Wall, "W", GROUND);
        wall.body = Some(extrusion(10_000.0, 200.0, 3000.0));
        wall
    }];
    let output = write(&model, &options()).unwrap();
    support::dump("wall-mm.ifc", &output.bytes);
    let text = std::str::from_utf8(&output.bytes).unwrap();
    assert!(text.contains("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)"));
    let (volumes, _) = volumes(&read_back(&output.bytes));
    assert!((volumes[&global_id(WALLS[0])] - 6.0).abs() < 1e-9);
}

#[test]
fn degenerate_bodies_and_empty_sets_are_reported_not_written() {
    let mut model = house();
    model.elements[0].body = Some(extrusion(10.0, 0.2, 0.0));
    model.elements[1].body = Some(Body::Brep {
        vertices: vec![Point3::origin()],
        faces: vec![vec![0, 1, 2]],
    });
    model.elements[2]
        .property_sets
        .push(PropertySet::new("Empty", Vec::new()));
    // Collinear points have no area.
    model.elements[4].body = Some(Body::Extrusion {
        outer: vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(2.0, 0.0),
        ],
        holes: Vec::new(),
        direction: Vector3::z(),
        depth: 1.0,
    });
    // A repeated index other than a closing one makes a face invalid.
    model.elements[7].body = Some(Body::Brep {
        vertices: vec![
            Point3::origin(),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ],
        faces: vec![vec![0, 1, 1, 2]],
    });
    // A closing repeat of the first index is dropped and the face is kept.
    if let Some(Body::Brep { faces, .. }) = &mut model.elements[3].body {
        let first = faces[0][0];
        faces[0].push(first);
    }
    let output = write(&model, &options()).unwrap();
    let kinds: Vec<_> = output
        .report
        .items
        .iter()
        .map(|i| {
            (
                i.reason.as_str(),
                i.record_kind.clone().unwrap_or_default(),
                i.arguments.get("count").cloned(),
            )
        })
        .collect();
    let count = |n| Some(arc_io_ifc::loss::Argument::Int(n));
    assert_eq!(
        kinds,
        [
            (
                "io.ifc.not_written",
                "IFCEXTRUDEDAREASOLID".to_owned(),
                count(2)
            ),
            ("io.ifc.not_written", "IFCFACETEDBREP".to_owned(), count(2)),
            ("io.ifc.not_written", "IFCPROPERTYSET".to_owned(), count(1)),
        ]
    );
    let text = String::from_utf8(output.bytes.clone()).unwrap();
    assert_eq!(text.matches("IFCFACETEDBREP(").count(), 1);
    let file = read_back(&output.bytes);
    let wall = file.instance(instance_of(&file, WALLS[0])).unwrap();
    assert_eq!(wall.records[0].parameters[6], Parameter::Omitted);
}

#[test]
fn invalid_models_are_refused_with_stable_codes() {
    let code = |model: &WriteModel| write(model, &options()).unwrap_err().code();
    let mut model = house();
    model.elements[0].storey = 0x999;
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    let mut model = house();
    model.elements[7].host = Some(WALLS[0]); // a door must fill an opening
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    let mut model = house();
    model.elements[1].id = WALLS[0];
    assert_eq!(code(&model), "io.ifc.write_id_duplicate");
    let mut model = house();
    model.elements[0].placement.origin.x = f64::NAN;
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[0].placement.ref_direction = Vector3::z();
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[0].quantity_sets[0].quantities[0].value = QuantityValue::Length(-1.0);
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[0].name = "a\0b".into();
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.true_north = Some(Vector2::new(0.0, 1e-200));
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[0].placement = Placement::new(
        Point3::origin(),
        Vector3::new(0.0, 0.0, 1e-200),
        Vector3::new(0.0, 1e-200, 1e-200),
    );
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[0].property_sets.push(PropertySet::new(
        "Qto_WallBaseQuantities",
        vec![Property::new("A", PropertyValue::Boolean(true))],
    ));
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house();
    model.elements[5].host = None; // an opening must void an element
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    let mut model = house();
    model.elements[5].host = Some(DOOR); // the opening is hosted by the door that fills it
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
}

#[test]
fn limits_are_respected() {
    let refuse = |limits: Limits, name: &str| {
        let options = WriteOptions {
            limits,
            ..options()
        };
        let error = write(&house(), &options).unwrap_err();
        assert_eq!(error.code(), "io.limit_exceeded");
        assert_eq!(
            error.arguments().get("limit"),
            Some(&arc_base::error::Value::from(name))
        );
    };
    refuse(
        Limits {
            record_count: 50,
            ..Limits::CONSTRAINED
        },
        "record_count",
    );
    refuse(
        Limits {
            string_bytes: 8,
            ..Limits::CONSTRAINED
        },
        "string_bytes",
    );
    refuse(
        Limits {
            array_length: 5,
            ..Limits::CONSTRAINED
        },
        "array_length",
    );
    refuse(
        Limits {
            memory_bytes: Some(2000),
            ..Limits::CONSTRAINED
        },
        "memory_bytes",
    );
    let cancel = Cancel::new();
    cancel.cancel();
    let options = WriteOptions {
        cancel,
        ..options()
    };
    assert_eq!(
        write(&house(), &options).unwrap_err().code(),
        "base.cancelled"
    );
}

const SPACES: [u128; 2] = [0x700, 0x701];

/// The house with two spaces on the ground storey: a rectangle and an L-shape with a hole.
fn house_with_spaces() -> WriteModel {
    let mut model = house();
    let mut room = Element::new(SPACES[0], ElementKind::Space, "101", GROUND);
    room.long_name = Some("Living \u{c7}".into());
    room.predefined_type = Some("internal".into());
    room.body = Some(extrusion(4.0, 3.0, 2.8));
    room.property_sets.push(PropertySet::new(
        "Pset_SpaceCommon",
        vec![Property::new(
            "Reference",
            PropertyValue::Label("101".into()),
        )],
    ));
    room.quantity_sets.push(QuantitySet::new(
        "Qto_SpaceBaseQuantities",
        vec![
            Quantity::new("NetFloorArea", QuantityValue::Area(12.0)),
            Quantity::new("NetVolume", QuantityValue::Volume(33.6)),
        ],
    ));
    let mut ell = Element::new(SPACES[1], ElementKind::Space, "102", GROUND);
    ell.placement = placed(5.0, 0.0, 0.0, Vector3::x());
    ell.body = Some(Body::Extrusion {
        outer: vec![
            Point2::new(0.0, 0.0),
            Point2::new(4.0, 0.0),
            Point2::new(4.0, 2.0),
            Point2::new(2.0, 2.0),
            Point2::new(2.0, 4.0),
            Point2::new(0.0, 4.0),
        ],
        holes: vec![vec![
            Point2::new(0.5, 0.5),
            Point2::new(0.5, 1.5),
            Point2::new(1.5, 1.5),
            Point2::new(1.5, 0.5),
        ]],
        direction: Vector3::z(),
        depth: 2.5,
    });
    model.elements.extend([room, ell]);
    model
}

#[test]
fn spaces_are_aggregated_by_their_storey_and_read_back() {
    let model = house_with_spaces();
    let output = write(&model, &options()).unwrap();
    support::dump("house-spaces.ifc", &output.bytes);
    assert_eq!(output.bytes, write(&model, &options()).unwrap().bytes);
    let file = read_back(&output.bytes);
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    assert!(!views.report.has_loss(), "{:?}", views.report);
    let storey = &views.spatial[0].children[0].children[0].children[0];
    let spaces: Vec<_> = storey
        .children
        .iter()
        .map(|n| (n.entity.type_name.as_str(), n.entity.name.clone().unwrap()))
        .collect();
    assert_eq!(
        spaces,
        [
            ("IFCSPACE", "101".to_owned()),
            ("IFCSPACE", "102".to_owned())
        ]
    );
    // Spaces are not contained in the storey.
    assert!(storey.contained.iter().all(|e| e.type_name != "IFCSPACE"));
    // LongName 7, CompositionType 8, PredefinedType 9, ElevationWithFlooring 10.
    let space = file.instance(instance_of(&file, SPACES[0])).unwrap();
    let parameters = &space.records[0].parameters;
    assert_eq!(parameters[7], Parameter::Text("Living \u{c7}".into()));
    assert_eq!(parameters[8], Parameter::Enumeration("ELEMENT".into()));
    assert_eq!(parameters[9], Parameter::Enumeration("INTERNAL".into()));
    assert_eq!(parameters[10], Parameter::Omitted);
    let ell = file.instance(instance_of(&file, SPACES[1])).unwrap();
    assert_eq!(ell.records[0].parameters[7], Parameter::Omitted);
    assert_eq!(
        ell.records[0].parameters[9],
        Parameter::Enumeration("NOTDEFINED".into())
    );
    let has_set = |n: &str| {
        views
            .property_sets
            .iter()
            .any(|p| p.definition.name.as_deref() == Some(n))
    };
    assert!(has_set("Pset_SpaceCommon"));
    assert!(
        views
            .quantity_sets
            .iter()
            .any(|q| q.definition.name.as_deref() == Some("Qto_SpaceBaseQuantities"))
    );
    let (volumes, _) = volumes(&file);
    let volume_of = |id| volumes.get(&global_id(id)).copied();
    assert!((volume_of(SPACES[0]).unwrap() - 33.6).abs() < 1e-9);
    // L-shape 12 m2 minus a 1 m2 hole, 2.5 m high.
    assert!((volume_of(SPACES[1]).unwrap() - 27.5).abs() < 1e-9);
}

#[test]
fn space_refusals_match_the_other_elements() {
    let code = |model: &WriteModel| write(model, &options()).unwrap_err().code();
    let mut model = house_with_spaces();
    model.elements[9].host = Some(WALLS[0]);
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    let mut model = house_with_spaces();
    if let Some(Body::Extrusion { depth, .. }) = &mut model.elements[9].body {
        *depth = f64::INFINITY;
    }
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house_with_spaces();
    model.elements[9].long_name = Some("a\0b".into());
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    // An empty profile is written without a body and reported.
    let mut model = house_with_spaces();
    model.elements[9].body = Some(Body::Extrusion {
        outer: Vec::new(),
        holes: Vec::new(),
        direction: Vector3::z(),
        depth: 2.0,
    });
    let output = write(&model, &options()).unwrap();
    support::dump("space-prism.ifc", &output.bytes);
    assert_eq!(output.report.items.len(), 1);
    assert_eq!(output.report.items[0].reason, "io.ifc.not_written");
}

const WALL_TYPE: u128 = 0x700;
const SLAB_TYPE: u128 = 0x701;
const DOOR_TYPE: u128 = 0x702;
const WINDOW_TYPE: u128 = 0x703;

/// The house with a layered wall type for its walls, a slab type with a membrane, and a door and a
/// window type.
fn house_with_types() -> WriteModel {
    let mut model = house();
    let mut wall = ElementType::new(WALL_TYPE, ElementKind::Wall, "Cavity 300");
    wall.predefined_type = Some("STANDARD".into());
    let mut brick = MaterialLayer::new("Brick", 0.1);
    brick.category = Some("Finish".into());
    let mut block = MaterialLayer::new("Concrete block", 0.15);
    block.category = Some("LoadBearing".into());
    wall.layers = vec![
        brick,
        MaterialLayer::new("Air", 0.05),
        block,
        MaterialLayer::new("Brick", 0.0),
    ];
    let mut slab = ElementType::new(SLAB_TYPE, ElementKind::Slab, "Slab 200");
    slab.predefined_type = Some("FLOOR".into());
    slab.layers = vec![
        MaterialLayer::new("Screed", 0.05),
        MaterialLayer::new("Membrane", 0.0),
        MaterialLayer::new("Concrete", 0.15),
    ];
    let mut door = ElementType::new(DOOR_TYPE, ElementKind::Door, "Double 1500");
    door.predefined_type = Some("DOOR".into());
    door.operation_type = Some("DOUBLE_DOOR_SINGLE_SWING".into());
    // Layers of a door type are not written.
    door.layers = vec![MaterialLayer::new("Wood", 0.04)];
    let mut window = ElementType::new(WINDOW_TYPE, ElementKind::Window, "Sliding");
    window.operation_type = Some("DOUBLE_PANEL_VERTICAL".into());
    model.types = vec![wall, slab, door, window];
    for element in &mut model.elements {
        match element.kind {
            ElementKind::Wall => {
                element.element_type = Some(WALL_TYPE);
                element.layer_usage = Some(LayerSetUsage::new(LayerDirection::Axis2, false, 0.15));
            }
            ElementKind::Slab => {
                element.element_type = Some(SLAB_TYPE);
                element.layer_usage = Some(LayerSetUsage::new(LayerDirection::Axis3, false, 0.0));
            }
            ElementKind::Door => element.element_type = Some(DOOR_TYPE),
            _ => {}
        }
    }
    model
}

#[test]
fn types_and_layer_sets_read_back() {
    let output = write(&house_with_types(), &options()).unwrap();
    support::dump("house-types.ifc", &output.bytes);
    let file = read_back(&output.bytes);
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    // Every wall, the slab and the door are typed; the window type has no occurrence.
    let assigned: Vec<(String, String)> = views
        .types
        .iter()
        .map(|t| {
            (
                t.element.global_id.clone().unwrap(),
                t.element_type.type_name.clone(),
            )
        })
        .collect();
    assert_eq!(assigned.len(), 6, "{assigned:?}");
    assert!(assigned.contains(&(global_id(WALLS[3]), "IFCWALLTYPE".to_owned())));
    assert!(assigned.contains(&(global_id(SLAB), "IFCSLABTYPE".to_owned())));
    assert!(assigned.contains(&(global_id(DOOR), "IFCDOORTYPE".to_owned())));
    assert_eq!(file.instances_of("IFCWINDOWTYPE").count(), 1);
    let door_type = file.instance(instance_of(&file, DOOR_TYPE)).unwrap();
    assert_eq!(
        door_type.records[0].parameters[10],
        Parameter::Enumeration("DOUBLE_DOOR_SINGLE_SWING".into())
    );

    // Two layer sets (the door's layers are reported), one material per name.
    assert_eq!(file.instances_of("IFCMATERIALLAYERSET").count(), 2);
    assert_eq!(file.instances_of("IFCMATERIALLAYER").count(), 7);
    assert_eq!(file.instances_of("IFCMATERIAL").count(), 6);
    let layer = file.instances_of("IFCMATERIALLAYER").next().unwrap();
    assert_eq!(layer.records[0].parameters[1], Parameter::Real(0.1));
    assert_eq!(
        layer.records[0].parameters[2],
        Parameter::Enumeration("U".into())
    );
    assert_eq!(
        layer.records[0].parameters[5],
        Parameter::Text("Finish".into())
    );
    let usages: Vec<&[Parameter]> = file
        .instances_of("IFCMATERIALLAYERSETUSAGE")
        .map(|i| &i.records[0].parameters[1..4])
        .collect();
    assert_eq!(usages.len(), 5);
    assert_eq!(
        usages[0],
        [
            Parameter::Enumeration("AXIS2".into()),
            Parameter::Enumeration("NEGATIVE".into()),
            Parameter::Real(0.15)
        ]
    );
    assert_eq!(usages[4][0], Parameter::Enumeration("AXIS3".into()));
    // Two types and five elements associate a material; the door's layers are the one loss.
    assert_eq!(file.instances_of("IFCRELASSOCIATESMATERIAL").count(), 7);
    assert_eq!(output.report.items.len(), 1, "{:?}", output.report);
    assert_eq!(
        output.report.items[0].record_kind.as_deref(),
        Some("IFCMATERIALLAYERSET")
    );
    assert_eq!(output.report.summary.mapped, 13);

    // The same model gives the same bytes.
    assert_eq!(
        write(&house_with_types(), &options()).unwrap().bytes,
        output.bytes
    );
}

#[test]
fn type_references_and_layers_are_checked() {
    let code = |model: &WriteModel| write(model, &options()).unwrap_err().code().to_owned();
    // A type of another kind, a missing type, a type of a space.
    let mut model = house_with_types();
    model.elements[0].element_type = Some(SLAB_TYPE);
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    model.elements[0].element_type = Some(0x799);
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    let mut model = house_with_types();
    model.types[3].kind = ElementKind::Space;
    assert_eq!(code(&model), "io.ifc.write_reference_invalid");
    // A type with an element's identifier.
    let mut model = house_with_types();
    model.types[3].id = WALLS[0];
    assert_eq!(code(&model), "io.ifc.write_id_duplicate");
    // A negative layer and an offset that is not a number.
    let mut model = house_with_types();
    model.types[0].layers[1].thickness = -0.01;
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    let mut model = house_with_types();
    model.elements[0].layer_usage = Some(LayerSetUsage::new(LayerDirection::Axis2, true, f64::NAN));
    assert_eq!(code(&model), "io.ifc.write_value_invalid");
    // A usage without a layer set is reported.
    let mut model = house_with_types();
    model.elements[0].element_type = None;
    let output = write(&model, &options()).unwrap();
    support::dump("house-types-no-layer-set.ifc", &output.bytes);
    assert!(
        output
            .report
            .items
            .iter()
            .any(|i| i.record_kind.as_deref() == Some("IFCMATERIALLAYERSETUSAGE"))
    );
}

fn file_voids(model: &WriteModel) -> usize {
    let output = write(model, &options()).unwrap();
    read_back(&output.bytes)
        .instances_of("IFCRELVOIDSELEMENT")
        .count()
}

const COVERING: u128 = 0x900;
const COVERING_TYPE: u128 = 0x901;
const COVERING_OPENING: u128 = 0x902;

#[test]
fn ceilings_are_coverings_with_a_type_a_layer_set_and_voids() {
    let mut model = house_with_types();
    let before = file_voids(&model);
    let mut covering_type = ElementType::new(COVERING_TYPE, ElementKind::Covering, "Board 20");
    covering_type.predefined_type = Some("ceiling".into());
    covering_type.layers = vec![MaterialLayer::new("Board", 0.02)];
    model.types.push(covering_type);
    let mut covering = Element::new(COVERING, ElementKind::Covering, "Ceiling 1", UPPER);
    covering.body = Some(box_brep(10.0, 6.0, 0.02));
    covering.predefined_type = Some("ceiling".into());
    covering.element_type = Some(COVERING_TYPE);
    model.elements.push(covering);
    let mut opening = Element::new(COVERING_OPENING, ElementKind::Opening, "Shaft", UPPER);
    opening.body = Some(box_brep(1.0, 1.0, 0.5));
    opening.host = Some(COVERING);
    model.elements.push(opening);
    let output = write(&model, &options()).unwrap();
    support::dump("house-ceilings.ifc", &output.bytes);
    let file = read_back(&output.bytes);
    let covering = file.instance(instance_of(&file, COVERING)).unwrap();
    assert_eq!(covering.records[0].type_name, "IFCCOVERING");
    assert_eq!(
        covering.records[0].parameters[8],
        Parameter::Enumeration("CEILING".into())
    );
    assert_eq!(file.instances_of("IFCCOVERINGTYPE").count(), 1);
    assert_eq!(file.instances_of("IFCRELVOIDSELEMENT").count(), before + 1);
    // A covering cannot have a host.
    if let Some(e) = model.elements.iter_mut().find(|e| e.id == COVERING) {
        e.host = Some(WALLS[0]);
    }
    assert_eq!(
        write(&model, &options()).unwrap_err().code(),
        "io.ifc.write_reference_invalid"
    );
}

const ROOF: u128 = 0x800;
const ROOF_TYPE: u128 = 0x801;

/// The house with a hip roof (a B-rep body) of a layered roof type on the upper storey.
fn house_with_roof() -> WriteModel {
    let mut model = house_with_types();
    let mut roof_type = ElementType::new(ROOF_TYPE, ElementKind::Roof, "Rafters 300");
    roof_type.predefined_type = Some("HIP_ROOF".into());
    roof_type.layers = vec![
        MaterialLayer::new("Tiles", 0.05),
        MaterialLayer::new("Insulation", 0.25),
    ];
    model.types.push(roof_type);
    let mut roof = Element::new(ROOF, ElementKind::Roof, "Roof 1", UPPER);
    roof.body = Some(box_brep(10.0, 6.0, 0.3));
    roof.predefined_type = Some("hip_roof".into());
    roof.element_type = Some(ROOF_TYPE);
    model.elements.push(roof);
    model
}

#[test]
fn roofs_and_roof_types_read_back() {
    let output = write(&house_with_roof(), &options()).unwrap();
    support::dump("house-roof.ifc", &output.bytes);
    let file = read_back(&output.bytes);
    let roof = file.instance(instance_of(&file, ROOF)).unwrap();
    assert_eq!(roof.records[0].type_name, "IFCROOF");
    assert_eq!(
        roof.records[0].parameters[8],
        Parameter::Enumeration("HIP_ROOF".into())
    );
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    assert!(views.types.iter().any(|t| {
        t.element.global_id.as_deref() == Some(global_id(ROOF).as_str())
            && t.element_type.type_name == "IFCROOFTYPE"
    }));
    // The roof type's layers are a third layer set; the roof has no usage of its own.
    assert_eq!(file.instances_of("IFCMATERIALLAYERSET").count(), 3);
    assert_eq!(file.instances_of("IFCMATERIALLAYERSETUSAGE").count(), 5);
    let (volumes, _) = volumes(&file);
    assert!((volumes[&global_id(ROOF)] - 18.0).abs() < 1e-9);
    // A roof cannot have a host.
    let mut model = house_with_roof();
    model.elements.last_mut().unwrap().host = Some(WALLS[0]);
    assert_eq!(
        write(&model, &options()).unwrap_err().code(),
        "io.ifc.write_reference_invalid"
    );
}
