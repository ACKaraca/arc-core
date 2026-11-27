//! IFC writing of the element kinds of the ArchBuilder families (furniture, sanitary terminals,
//! light fixtures, electric appliances, unitary equipment, columns, beams and building element
//! proxies): read back with their types and predefined values; property and quantity sets on
//! element types; shared geometry through representation maps and mapped items; and aggregation
//! of any element through its host.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

mod support;

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Cancel, Limits};
use arc_io_ifc::schema::SchemaTables;
use arc_io_ifc::write::{
    Body, Element, ElementKind, ElementType, OutputSchema, Placement, Property, PropertySet,
    PropertyValue, Quantity, QuantitySet, QuantityValue, Spatial, Storey, WriteModel, WriteOptions,
};
use arc_io_ifc::{IfcFile, Parameter, ReadOptions, Schema, codes, read, write};
use arc_mesh::measure::{bounds, volume};

const STOREY: u128 = 0x100;
const WALL: u128 = 0x200;
const OPENING: u128 = 0x201;
const SPACE: u128 = 0x202;
const DESK: u128 = 0x1000;
const LAMP: u128 = 0x1001;
const KETTLE: u128 = 0x1002;

fn rectangle(x: f64, y: f64) -> Vec<Point2> {
    vec![
        Point2::new(0.0, 0.0),
        Point2::new(x, 0.0),
        Point2::new(x, y),
        Point2::new(0.0, y),
    ]
}

/// A box of `x` by `y` by `z`, from the origin of its own coordinates.
fn box_body(x: f64, y: f64, z: f64) -> Body {
    Body::Extrusion {
        outer: rectangle(x, y),
        holes: Vec::new(),
        direction: Vector3::z(),
        depth: z,
    }
}

fn base() -> WriteModel {
    let mut model = WriteModel::new(
        Spatial::new(1, "Project"),
        Spatial::new(2, "Site"),
        Spatial::new(3, "Building"),
    );
    model.storeys.push(Storey::new(STOREY, "Ground", 0.0));
    model
}

fn element(id: u128, kind: ElementKind, name: &str, predefined: Option<&str>) -> Element {
    let mut e = Element::new(id, kind, name, STOREY);
    e.predefined_type = predefined.map(str::to_owned);
    e.body = Some(box_body(0.5, 0.5, 0.9));
    e
}

fn written(model: &WriteModel, name: &str) -> (Vec<u8>, IfcFile) {
    let output = write(model, &WriteOptions::default()).unwrap();
    support::dump(name, &output.bytes);
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    (output.bytes, file)
}

/// The code of the error the writer refuses the model with.
fn refused(model: &WriteModel) -> &'static str {
    write(model, &WriteOptions::default()).unwrap_err().code()
}

/// The reference of a parameter that holds one instance.
fn reference_of(parameter: &Parameter) -> u64 {
    let Parameter::Reference(name) = parameter else {
        panic!("not a reference: {parameter:?}")
    };
    *name
}

fn list_of(parameter: &Parameter) -> Vec<u64> {
    let Parameter::List(items) = parameter else {
        panic!("not a list: {parameter:?}")
    };
    items.iter().map(reference_of).collect()
}

/// A kind, its element and type entities, its enumeration, the values the writer names and the
/// one of them each case writes.
type Case = (
    ElementKind,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
);

#[test]
fn new_kinds_read_back_with_their_types_and_predefined_values() {
    let cases: [Case; 8] = [
        (
            ElementKind::Furniture,
            "IFCFURNITURE",
            "IFCFURNITURETYPE",
            "IfcFurnitureTypeEnum",
            &[
                "CHAIR",
                "TABLE",
                "DESK",
                "BED",
                "FILECABINET",
                "SHELF",
                "SOFA",
            ],
            "FILECABINET",
        ),
        (
            ElementKind::SanitaryTerminal,
            "IFCSANITARYTERMINAL",
            "IFCSANITARYTERMINALTYPE",
            "IfcSanitaryTerminalTypeEnum",
            &["SINK", "TOILETPAN", "SHOWER", "WCSEAT"],
            "WCSEAT",
        ),
        (
            ElementKind::LightFixture,
            "IFCLIGHTFIXTURE",
            "IFCLIGHTFIXTURETYPE",
            "IfcLightFixtureTypeEnum",
            &["POINTSOURCE", "DIRECTIONSOURCE", "SECURITYLIGHTING"],
            "DIRECTIONSOURCE",
        ),
        (
            ElementKind::ElectricAppliance,
            "IFCELECTRICAPPLIANCE",
            "IFCELECTRICAPPLIANCETYPE",
            "IfcElectricApplianceTypeEnum",
            &["MICROWAVE", "FRIDGE_FREEZER", "WASHINGMACHINE"],
            "FRIDGE_FREEZER",
        ),
        (
            ElementKind::UnitaryEquipment,
            "IFCUNITARYEQUIPMENT",
            "IFCUNITARYEQUIPMENTTYPE",
            "IfcUnitaryEquipmentTypeEnum",
            &["AIRHANDLER", "ROOFTOPUNIT", "SPLITSYSTEM"],
            "SPLITSYSTEM",
        ),
        (
            ElementKind::Column,
            "IFCCOLUMN",
            "IFCCOLUMNTYPE",
            "IfcColumnTypeEnum",
            &["COLUMN", "PILASTER"],
            "PILASTER",
        ),
        (
            ElementKind::Beam,
            "IFCBEAM",
            "IFCBEAMTYPE",
            "IfcBeamTypeEnum",
            &[
                "BEAM",
                "JOIST",
                "HOLLOWCORE",
                "LINTEL",
                "SPANDREL",
                "T_BEAM",
            ],
            "T_BEAM",
        ),
        (
            ElementKind::BuildingElementProxy,
            "IFCBUILDINGELEMENTPROXY",
            "IFCBUILDINGELEMENTPROXYTYPE",
            "IfcBuildingElementProxyTypeEnum",
            &[
                "COMPLEX",
                "ELEMENT",
                "PARTIAL",
                "PROVISIONFORVOID",
                "PROVISIONFORSPACE",
            ],
            "PARTIAL",
        ),
    ];
    let tables = SchemaTables::of(&Schema::Ifc4);
    for (index, (kind, entity, type_entity, enumeration, values, chosen)) in
        cases.into_iter().enumerate()
    {
        if let Some(tables) = tables {
            let items = tables.enumeration(enumeration).unwrap();
            for value in values {
                assert!(items.contains(value), "{enumeration} {value}");
            }
        }
        let mut model = base();
        let index = u128::try_from(index).unwrap();
        let mut occurrence = element(0x1000 + index, kind, "Item", Some(chosen));
        occurrence.element_type = Some(0x2000 + index);
        model.elements.push(occurrence);
        let mut element_type = ElementType::new(0x2000 + index, kind, "Type");
        element_type.predefined_type = Some(chosen.to_owned());
        model.types.push(element_type);
        let (_, file) = written(&model, &format!("write_kinds_{entity}.ifc"));

        let instance = file.instances_of(entity).next().unwrap();
        assert_eq!(instance.records[0].parameters.len(), 9, "{entity}");
        assert_eq!(
            instance.records[0].parameters[8],
            Parameter::Enumeration(chosen.into()),
            "{entity}"
        );
        let element_type = file.instances_of(type_entity).next().unwrap();
        let parameters = &element_type.records[0].parameters;
        // The furniture type has `AssemblyPlace` (mandatory in IFC4) before its predefined type.
        let (predefined, expected_len) = if kind == ElementKind::Furniture {
            assert_eq!(parameters[9], Parameter::Enumeration("NOTDEFINED".into()));
            (10, 11)
        } else {
            (9, 10)
        };
        assert_eq!(parameters.len(), expected_len, "{type_entity}");
        assert_eq!(
            parameters[predefined],
            Parameter::Enumeration(chosen.into()),
            "{type_entity}"
        );
        let views = file
            .typed_views(&Limits::CONSTRAINED, &Cancel::new())
            .unwrap();
        assert!(views.types.iter().any(|t| {
            t.element.instance == instance.name && t.element_type.instance == element_type.name
        }));
        let contained = views.spatial[0].children[0].children[0].children[0]
            .contained
            .iter()
            .any(|e| e.instance == instance.name);
        assert!(contained, "{entity} is contained in its storey");
    }
}

#[test]
fn building_element_proxy_is_written_only_when_the_model_asks_for_it() {
    let mut model = base();
    model
        .elements
        .push(element(DESK, ElementKind::Furniture, "Desk", Some("DESK")));
    let (bytes, _) = written(&model, "write_kinds_no_proxy.ifc");
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("IFCBUILDINGELEMENTPROXY")
    );
}

#[test]
fn type_property_and_quantity_sets_are_listed_by_the_type() {
    let mut model = base();
    let mut column_type = ElementType::new(0x2100, ElementKind::Column, "Concrete 300");
    column_type.predefined_type = Some("COLUMN".into());
    column_type.property_sets = vec![
        PropertySet::new(
            "Pset_ColumnCommon",
            vec![
                Property::new("Reference", PropertyValue::identifier("C-300").unwrap()),
                Property::new(
                    "Width",
                    PropertyValue::measure(arc_io_ifc::write::Measure::PositiveLength, 0.3)
                        .unwrap(),
                ),
            ],
        ),
        PropertySet::new("Pset_Empty", Vec::new()),
    ];
    column_type.quantity_sets = vec![QuantitySet::new(
        "Qto_ColumnBaseQuantities",
        vec![Quantity::new("Length", QuantityValue::Length(3.0))],
    )];
    model.types.push(column_type);
    let mut column = element(0x1100, ElementKind::Column, "Column", Some("COLUMN"));
    column.element_type = Some(0x2100);
    model.elements.push(column);
    let (_, file) = written(&model, "write_kinds_type_sets.ifc");

    let column_type = file.instances_of("IFCCOLUMNTYPE").next().unwrap();
    // Every set of the type is listed in `HasPropertySets`; the empty one is not written.
    let listed = list_of(&column_type.records[0].parameters[5]);
    assert_eq!(listed.len(), 2);
    let names: Vec<String> = listed
        .iter()
        .map(|name| {
            let set = file
                .instances_of("IFCPROPERTYSET")
                .chain(file.instances_of("IFCELEMENTQUANTITY"))
                .find(|i| i.name == *name)
                .expect("a listed set is written");
            let Parameter::Text(text) = &set.records[0].parameters[2] else {
                panic!("set name")
            };
            text.clone()
        })
        .collect();
    assert_eq!(names, ["Pset_ColumnCommon", "Qto_ColumnBaseQuantities"]);
    // The owner of a type's sets is the type: no `IfcRelDefinesByProperties` relates them.
    assert_eq!(file.instances_of("IFCRELDEFINESBYPROPERTIES").count(), 0);
    assert_eq!(
        column_type.records[0].parameters[6],
        Parameter::Omitted,
        "no maps"
    );
}

#[test]
fn mapped_items_share_the_type_representation_with_their_own_transform() {
    let mut model = base();
    let mut chair = ElementType::new(0x2200, ElementKind::Furniture, "Chair");
    chair.predefined_type = Some("CHAIR".into());
    chair.representation = Some(box_body(0.5, 0.5, 0.9));
    model.types.push(chair);
    let occurrences = [
        // Placed at the storey origin, mapped without a transform.
        Placement::default(),
        // Translated by two meters along x.
        Placement::new(Point3::new(2.0, 0.0, 0.0), Vector3::z(), Vector3::x()),
        // Translated and turned a quarter turn about z: its local x axis is the world y axis.
        Placement::new(Point3::new(4.0, 1.0, 0.0), Vector3::z(), Vector3::y()),
    ];
    for (index, transform) in occurrences.into_iter().enumerate() {
        let mut item = Element::new(
            0x2300 + u128::try_from(index).unwrap(),
            ElementKind::Furniture,
            "Chair",
            STOREY,
        );
        item.predefined_type = Some("CHAIR".into());
        item.element_type = Some(0x2200);
        item.mapping = Some(transform);
        model.elements.push(item);
    }
    let (_, file) = written(&model, "write_kinds_mapped.ifc");

    assert_eq!(file.instances_of("IFCREPRESENTATIONMAP").count(), 1);
    assert_eq!(file.instances_of("IFCMAPPEDITEM").count(), 3);
    assert_eq!(
        file.instances_of("IFCCARTESIANTRANSFORMATIONOPERATOR3D")
            .count(),
        3
    );
    let chair_type = file.instances_of("IFCFURNITURETYPE").next().unwrap();
    assert_eq!(list_of(&chair_type.records[0].parameters[6]).len(), 1);

    // Each occurrence evaluates the shared box at its own transform, in the same volume.
    let geometry = file.geometry(&Limits::CONSTRAINED, &Cancel::new()).unwrap();
    let products: Vec<u64> = file.instances_of("IFCFURNITURE").map(|i| i.name).collect();
    assert_eq!(products.len(), 3);
    let mut lows = Vec::new();
    for product in &products {
        let item = geometry
            .items
            .iter()
            .find(|item| item.product == *product)
            .expect("each occurrence has geometry");
        let mesh = item.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap();
        let measured = volume(&mesh, &Limits::CONSTRAINED).unwrap();
        assert!(
            (measured - 0.5 * 0.5 * 0.9).abs() < 1e-9,
            "volume {measured}"
        );
        let aabb = bounds(&mesh);
        lows.push((aabb.min().x, aabb.min().y));
    }
    let expected = [(0.0, 0.0), (2.0, 0.0), (3.5, 1.0)];
    for (low, want) in lows.iter().zip(expected) {
        assert!(
            (low.0 - want.0).abs() < 1e-9 && (low.1 - want.1).abs() < 1e-9,
            "{low:?} {want:?}"
        );
    }
}

#[test]
fn mapping_needs_a_type_with_a_representation_and_no_body() {
    let mut base_model = base();
    let mut plain = ElementType::new(0x2400, ElementKind::Furniture, "Plain");
    plain.predefined_type = Some("TABLE".into());
    base_model.types.push(plain);
    let mapped = |body: bool, type_id: Option<u128>, origin: Point3| {
        let mut model = base_model.clone();
        let mut item = Element::new(0x2500, ElementKind::Furniture, "Item", STOREY);
        if body {
            item.body = Some(box_body(0.5, 0.5, 0.9));
        }
        item.element_type = type_id;
        item.mapping = Some(Placement::new(origin, Vector3::z(), Vector3::x()));
        model.elements.push(item);
        model
    };
    let origin = Point3::origin();
    // A mapping without a type.
    assert_eq!(
        refused(&mapped(false, None, origin)),
        codes::IFC_WRITE_VALUE_INVALID.code
    );
    // A mapping of a type without a representation.
    assert_eq!(
        refused(&mapped(false, Some(0x2400), origin)),
        codes::IFC_WRITE_REFERENCE_INVALID.code
    );
    // A mapping and the element's own body.
    let mut both = base();
    let mut typed = ElementType::new(0x2400, ElementKind::Furniture, "Shared");
    typed.predefined_type = Some("TABLE".into());
    typed.representation = Some(box_body(0.5, 0.5, 0.9));
    both.types.push(typed);
    let mut item = Element::new(0x2500, ElementKind::Furniture, "Item", STOREY);
    item.body = Some(box_body(0.5, 0.5, 0.9));
    item.element_type = Some(0x2400);
    item.mapping = Some(Placement::default());
    both.elements.push(item);
    assert_eq!(refused(&both), codes::IFC_WRITE_VALUE_INVALID.code);
    // A transform that is not finite, and axes that are parallel.
    let mut shared = base();
    let mut typed = ElementType::new(0x2400, ElementKind::Furniture, "Shared");
    typed.predefined_type = Some("TABLE".into());
    typed.representation = Some(box_body(0.5, 0.5, 0.9));
    shared.types.push(typed);
    let mut item = Element::new(0x2500, ElementKind::Furniture, "Item", STOREY);
    item.element_type = Some(0x2400);
    item.mapping = Some(Placement::new(
        Point3::new(f64::NAN, 0.0, 0.0),
        Vector3::z(),
        Vector3::x(),
    ));
    shared.elements.push(item.clone());
    assert_eq!(refused(&shared), codes::IFC_WRITE_VALUE_INVALID.code);
    shared.elements.pop();
    item.mapping = Some(Placement::new(origin, Vector3::z(), Vector3::z()));
    shared.elements.push(item);
    assert_eq!(refused(&shared), codes::IFC_WRITE_VALUE_INVALID.code);
}

/// A desk with a lamp and a kettle on it: both are aggregated by the desk, a furniture element.
fn desk_model() -> WriteModel {
    let mut model = base();
    model
        .elements
        .push(element(DESK, ElementKind::Furniture, "Desk", Some("DESK")));
    let mut lamp = element(LAMP, ElementKind::LightFixture, "Lamp", Some("POINTSOURCE"));
    lamp.host = Some(DESK);
    model.elements.push(lamp);
    let mut kettle = element(
        KETTLE,
        ElementKind::ElectricAppliance,
        "Kettle",
        Some("KITCHENMACHINE"),
    );
    kettle.host = Some(DESK);
    model.elements.push(kettle);
    model
}

#[test]
fn any_element_is_aggregated_by_its_host_and_not_contained() {
    let (_, file) = written(&desk_model(), "write_kinds_aggregation.ifc");
    let desk = file.instances_of("IFCFURNITURE").next().unwrap().name;
    // The spatial structure aggregates too; the desk's aggregation is the one of its parts.
    let parts: Vec<Vec<u64>> = file
        .instances_of("IFCRELAGGREGATES")
        .filter(|r| reference_of(&r.records[0].parameters[4]) == desk)
        .map(|r| list_of(&r.records[0].parameters[5]))
        .collect();
    assert_eq!(parts.len(), 1, "one aggregation of the desk");
    assert_eq!(parts[0].len(), 2, "the lamp and the kettle");
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    let contained: Vec<u64> = views.spatial[0].children[0].children[0].children[0]
        .contained
        .iter()
        .map(|e| e.instance)
        .collect();
    assert_eq!(
        contained,
        [desk],
        "the desk alone is contained in the storey"
    );
}

#[test]
fn aggregation_refuses_invalid_hosts_and_cycles() {
    let wall = |model: &mut WriteModel| {
        model
            .elements
            .push(element(WALL, ElementKind::Wall, "Wall", None));
    };
    // A host that is not in the model.
    let mut missing = desk_model();
    missing.elements[1].host = Some(0x9999);
    assert_eq!(refused(&missing), codes::IFC_WRITE_REFERENCE_INVALID.code);
    // A host that is an opening or a space.
    let mut opening = desk_model();
    wall(&mut opening);
    let mut void = Element::new(OPENING, ElementKind::Opening, "Void", STOREY);
    void.host = Some(WALL);
    opening.elements.push(void);
    opening.elements[1].host = Some(OPENING);
    assert_eq!(refused(&opening), codes::IFC_WRITE_REFERENCE_INVALID.code);
    let mut space = desk_model();
    space
        .elements
        .push(Element::new(SPACE, ElementKind::Space, "Room", STOREY));
    space.elements[1].host = Some(SPACE);
    assert_eq!(refused(&space), codes::IFC_WRITE_REFERENCE_INVALID.code);
    // A host cycle, and an element hosting itself.
    let mut cycle = desk_model();
    cycle.elements[0].host = Some(LAMP);
    assert_eq!(refused(&cycle), codes::IFC_WRITE_REFERENCE_INVALID.code);
    let mut itself = desk_model();
    itself.elements[0].host = Some(DESK);
    assert_eq!(refused(&itself), codes::IFC_WRITE_REFERENCE_INVALID.code);
    // A wall keeps its rule: it has no host.
    let mut walled = desk_model();
    let mut hosted_wall = element(WALL, ElementKind::Wall, "Wall", None);
    hosted_wall.host = Some(DESK);
    walled.elements.push(hosted_wall);
    assert_eq!(refused(&walled), codes::IFC_WRITE_REFERENCE_INVALID.code);
}

#[cfg(arc_ifc_tables_ifc2x3)]
#[test]
fn ifc2x3_reports_the_furniture_it_cannot_carry_and_never_substitutes_a_proxy() {
    let mut model = desk_model();
    model.elements.push(element(
        0x1200,
        ElementKind::Column,
        "Column",
        Some("COLUMN"),
    ));
    model
        .elements
        .push(element(0x1201, ElementKind::Beam, "Beam", Some("BEAM")));
    let options = WriteOptions {
        schema: OutputSchema::Ifc2x3,
        ..WriteOptions::default()
    };
    let output = write(&model, &options).unwrap();
    support::dump("write_kinds_ifc2x3.ifc", &output.bytes);
    let text = String::from_utf8(output.bytes.clone()).unwrap();
    assert!(!text.contains("IFCFURNITURE"));
    assert!(!text.contains("IFCBUILDINGELEMENTPROXY"));
    assert!(text.contains("IFCCOLUMN("));
    assert!(text.contains("IFCBEAM("));
    let reported = output
        .report
        .items
        .iter()
        .any(|i| i.record_kind.as_deref() == Some("IFCFURNITURE"));
    assert!(reported, "the furniture is reported");
}

#[test]
fn mapped_and_aggregated_kinds_conform_in_every_schema() {
    let mut model = desk_model();
    let mut column_type = ElementType::new(0x2600, ElementKind::Column, "Steel column");
    column_type.predefined_type = Some("COLUMN".into());
    column_type.representation = Some(box_body(0.3, 0.3, 3.0));
    model.types.push(column_type);
    let mut column = Element::new(0x1300, ElementKind::Column, "Column", STOREY);
    column.predefined_type = Some("COLUMN".into());
    column.element_type = Some(0x2600);
    column.mapping = Some(Placement::new(
        Point3::new(3.0, 0.0, 0.0),
        Vector3::z(),
        Vector3::x(),
    ));
    model.elements.push(column);
    for schema in [
        OutputSchema::Ifc4,
        OutputSchema::Ifc4x3,
        OutputSchema::Ifc2x3,
    ] {
        if !schema.available() {
            continue;
        }
        let options = WriteOptions {
            schema,
            ..WriteOptions::default()
        };
        let output = write(&model, &options).unwrap();
        let name = format!("write_kinds_mapped_{}.ifc", schema.identifier());
        // Dumping checks the conformance of the file with the schema tables.
        support::dump(&name, &output.bytes);
        assert!(
            output.findings.is_empty(),
            "{schema:?}: {:?}",
            output.findings
        );
        let text = String::from_utf8(output.bytes).unwrap();
        assert!(text.contains("IFCMAPPEDITEM("), "{schema:?}");
        assert!(text.contains("IFCRELAGGREGATES("), "{schema:?}");
    }
}
