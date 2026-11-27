//! IFC writing of the property and quantity value kinds the published templates use, of
//! transport elements and pipe segments, groups, georeferencing and polyline and composite grid
//! axes. Every written file is read back; where the generated templates exist, every standard set
//! must match its template.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

mod support;

use std::f64::consts::{FRAC_PI_2, FRAC_PI_6, PI};

use arc_base::math::{Point2, Vector2, Vector3};
use arc_base::{Cancel, Limits};
use arc_io_ifc::psets::PsetTemplates;
use arc_io_ifc::schema::SchemaTables;
use arc_io_ifc::write::{
    AngleUnit, AxisCurve, AxisSegment, Body, Element, ElementKind, ElementType, Georeference, Grid,
    GridAxis, Group, Measure, Property, PropertySet, PropertyValue, Quantity, QuantitySet,
    QuantityValue, Spatial, Storey,
};
use arc_io_ifc::{
    IfcFile, Nominal, Parameter, PropertyValue as ReadValue, QuantityKind, ReadOptions, Schema,
    Value, WriteModel, WriteOptions, codes, read, write,
};

const STOREY: u128 = 0x10;
const STATUS: [&str; 7] = [
    "NEW",
    "EXISTING",
    "DEMOLISH",
    "TEMPORARY",
    "OTHER",
    "NOTKNOWN",
    "UNSET",
];

fn body() -> Option<Body> {
    Some(Body::Extrusion {
        outer: vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ],
        holes: Vec::new(),
        direction: Vector3::z(),
        depth: 1.0,
    })
}

fn model() -> WriteModel {
    let mut model = WriteModel::new(
        Spatial::new(1, "Project"),
        Spatial::new(2, "Site"),
        Spatial::new(3, "Building"),
    );
    model.storeys.push(Storey::new(STOREY, "Ground", 0.0));
    model
}

fn element(id: u128, kind: ElementKind, predefined: Option<&str>) -> Element {
    let mut e = Element::new(id, kind, &format!("{kind:?}"), STOREY);
    e.predefined_type = predefined.map(str::to_owned);
    e.body = body();
    e
}

fn p(name: &str, value: PropertyValue) -> Property {
    Property::new(name, value)
}

fn m(measure: Measure, value: f64) -> PropertyValue {
    PropertyValue::measure(measure, value).unwrap()
}

fn status() -> PropertyValue {
    PropertyValue::enumerated("PEnum_ElementStatus", &STATUS, &["NEW"]).unwrap()
}

fn written(model: &WriteModel, name: &str) -> (Vec<u8>, IfcFile) {
    let output = write(model, &WriteOptions::default()).unwrap();
    support::dump(name, &output.bytes);
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    (output.bytes, file)
}

/// Requires that every standard set of the file matches its template, where the templates exist.
fn templates_match(file: &IfcFile, expected_sets: u64) {
    let check = file
        .check_property_sets(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    if PsetTemplates::of(&Schema::Ifc4).is_none() {
        assert_eq!(check.templates, None);
        return;
    }
    assert_eq!(
        check.findings,
        [],
        "standard sets differ from their templates"
    );
    assert_eq!(check.sets_checked, expected_sets);
}

/// A building with an element of every kind ArchBuilder exports, each with its common property
/// set and base quantities as the published templates define them.
fn standard_model() -> (WriteModel, u64) {
    let mut model = model();
    model.angle_unit = AngleUnit::Degree;
    let mut sets = 0;
    let mut add = |mut e: Element, psets: Vec<PropertySet>, qtos: Vec<QuantitySet>| {
        sets += (psets.len() + qtos.len()) as u64;
        e.property_sets = psets;
        e.quantity_sets = qtos;
        model.elements.push(e);
    };
    let common = |extra: Vec<Property>| {
        let mut list = vec![
            p("Reference", PropertyValue::identifier("T-1").unwrap()),
            p("Status", status()),
        ];
        list.extend(extra);
        list
    };
    add(
        element(0x100, ElementKind::Wall, Some("SOLIDWALL")),
        vec![PropertySet::new(
            "Pset_WallCommon",
            common(vec![
                p("AcousticRating", PropertyValue::label("R'w 52").unwrap()),
                p("FireRating", PropertyValue::Label("REI 90".into())),
                p(
                    "ThermalTransmittance",
                    m(Measure::ThermalTransmittance, 0.28),
                ),
                p("IsExternal", PropertyValue::Boolean(true)),
                p("LoadBearing", PropertyValue::Boolean(true)),
            ]),
        )],
        vec![QuantitySet::new(
            "Qto_WallBaseQuantities",
            vec![
                Quantity::new("Length", QuantityValue::Length(1.0)),
                Quantity::new("Width", QuantityValue::Length(1.0)),
                Quantity::new("GrossFootprintArea", QuantityValue::Area(1.0)),
                Quantity::new("NetVolume", QuantityValue::Volume(1.0)),
                Quantity::new("NetWeight", QuantityValue::Weight(450.0)),
            ],
        )],
    );
    add(
        element(0x101, ElementKind::Slab, Some("FLOOR")),
        vec![PropertySet::new(
            "Pset_SlabCommon",
            common(vec![
                p("PitchAngle", m(Measure::PlaneAngle, FRAC_PI_6)),
                p("IsExternal", PropertyValue::Boolean(false)),
            ]),
        )],
        vec![QuantitySet::new(
            "Qto_SlabBaseQuantities",
            vec![
                Quantity::new("Depth", QuantityValue::Length(0.2)),
                Quantity::new("GrossArea", QuantityValue::Area(1.0)),
                Quantity::new("GrossWeight", QuantityValue::Weight(500.0)),
            ],
        )],
    );
    add(
        element(0x102, ElementKind::Roof, Some("FLAT_ROOF")),
        vec![PropertySet::new(
            "Pset_RoofCommon",
            common(vec![
                p("IsExternal", PropertyValue::Boolean(true)),
                // The published definition leaves the type open.
                p("LoadBearing", PropertyValue::Boolean(true)),
            ]),
        )],
        vec![QuantitySet::new(
            "Qto_RoofBaseQuantities",
            vec![Quantity::new("GrossArea", QuantityValue::Area(1.0))],
        )],
    );
    add(
        element(0x103, ElementKind::Covering, Some("CEILING")),
        vec![PropertySet::new(
            "Pset_CoveringCommon",
            common(vec![p(
                "Finish",
                PropertyValue::Text("Painted, two coats".into()),
            )]),
        )],
        vec![],
    );
    add(
        element(0x104, ElementKind::Space, Some("INTERNAL")),
        vec![
            PropertySet::new(
                "Pset_SpaceCommon",
                vec![
                    p("Reference", PropertyValue::Identifier("101".into())),
                    p("NetPlannedArea", m(Measure::Area, 12.5)),
                    p("HandicapAccessible", PropertyValue::Boolean(true)),
                ],
            ),
            PropertySet::new(
                "Pset_SpaceThermalRequirements",
                vec![p("SpaceHumidity", m(Measure::Ratio, 0.45))],
            ),
        ],
        vec![QuantitySet::new(
            "Qto_SpaceBaseQuantities",
            vec![Quantity::new("NetFloorArea", QuantityValue::Area(12.5))],
        )],
    );
    add(
        element(0x105, ElementKind::Stair, Some("STRAIGHT_RUN_STAIR")),
        vec![PropertySet::new(
            "Pset_StairCommon",
            common(vec![
                p("NumberOfRiser", m(Measure::Count, 16.0)),
                p("RiserHeight", m(Measure::PositiveLength, 0.175)),
                p("NosingLength", PropertyValue::Length(0.02)),
                p("RequiredHeadroom", m(Measure::PositiveLength, 2.1)),
            ]),
        )],
        vec![],
    );
    let mut flight = element(0x106, ElementKind::StairFlight, Some("STRAIGHT"));
    flight.host = Some(0x105);
    add(
        flight,
        vec![PropertySet::new(
            "Pset_StairFlightCommon",
            common(vec![p("TreadLength", m(Measure::PositiveLength, 0.28))]),
        )],
        vec![QuantitySet::new(
            "Qto_StairFlightBaseQuantities",
            vec![Quantity::new("Length", QuantityValue::Length(4.5))],
        )],
    );
    let mut railing = element(0x107, ElementKind::Railing, Some("HANDRAIL"));
    railing.host = Some(0x105);
    add(
        railing,
        vec![PropertySet::new(
            "Pset_RailingCommon",
            common(vec![
                p("Height", m(Measure::PositiveLength, 0.9)),
                p("Diameter", m(Measure::PositiveLength, 0.04)),
            ]),
        )],
        vec![QuantitySet::new(
            "Qto_RailingBaseQuantities",
            vec![Quantity::new("Length", QuantityValue::Length(4.5))],
        )],
    );
    add(
        element(0x108, ElementKind::Ramp, Some("STRAIGHT_RUN_RAMP")),
        vec![PropertySet::new(
            "Pset_RampCommon",
            common(vec![
                p("RequiredSlope", m(Measure::PlaneAngle, 0.1)),
                p("HandicapAccessible", PropertyValue::Boolean(true)),
            ]),
        )],
        vec![],
    );
    let mut ramp_flight = element(0x109, ElementKind::RampFlight, Some("STRAIGHT"));
    ramp_flight.host = Some(0x108);
    add(
        ramp_flight,
        vec![PropertySet::new(
            "Pset_RampFlightCommon",
            common(vec![
                p("ClearWidth", m(Measure::PositiveLength, 1.2)),
                p("Slope", m(Measure::PlaneAngle, 0.08)),
            ]),
        )],
        vec![],
    );
    add(
        element(0x10A, ElementKind::CurtainWall, None),
        vec![PropertySet::new(
            "Pset_CurtainWallCommon",
            common(vec![p("IsExternal", PropertyValue::Boolean(true))]),
        )],
        vec![],
    );
    let mut plate = element(0x10B, ElementKind::Plate, Some("CURTAIN_PANEL"));
    plate.host = Some(0x10A);
    add(
        plate,
        vec![PropertySet::new(
            "Pset_PlateCommon",
            common(vec![p("LoadBearing", PropertyValue::Boolean(false))]),
        )],
        vec![QuantitySet::new(
            "Qto_PlateBaseQuantities",
            vec![Quantity::new("Width", QuantityValue::Length(0.02))],
        )],
    );
    let mut member = element(0x10C, ElementKind::Member, Some("MULLION"));
    member.host = Some(0x10A);
    add(
        member,
        vec![PropertySet::new(
            "Pset_MemberCommon",
            common(vec![
                p("Span", m(Measure::PositiveLength, 3.0)),
                p("Slope", m(Measure::PlaneAngle, FRAC_PI_2)),
            ]),
        )],
        vec![QuantitySet::new(
            "Qto_MemberBaseQuantities",
            vec![Quantity::new("Length", QuantityValue::Length(3.0))],
        )],
    );
    let mut opening = element(0x10D, ElementKind::Opening, Some("OPENING"));
    opening.host = Some(0x100);
    add(opening, vec![], vec![]);
    let mut door = element(0x10E, ElementKind::Door, Some("DOOR"));
    door.host = Some(0x10D);
    door.overall_width = Some(0.9);
    door.overall_height = Some(2.1);
    add(
        door,
        vec![
            PropertySet::new(
                "Pset_DoorCommon",
                common(vec![
                    p("FireExit", PropertyValue::Boolean(true)),
                    p("GlazingAreaFraction", m(Measure::PositiveRatio, 0.2)),
                    p("Infiltration", m(Measure::VolumetricFlowRate, 0.001)),
                ]),
            ),
            PropertySet::new(
                "Pset_DoorWindowGlazingType",
                vec![p(
                    "VisibleLightTransmittance",
                    m(Measure::NormalisedRatio, 0.7),
                )],
            ),
        ],
        vec![QuantitySet::new(
            "Qto_DoorBaseQuantities",
            vec![
                Quantity::new("Width", QuantityValue::Length(0.9)),
                Quantity::new("Area", QuantityValue::Area(1.89)),
            ],
        )],
    );
    let mut lift = element(0x10F, ElementKind::TransportElement, Some("elevator"));
    lift.element_type = Some(0x200);
    add(
        lift,
        vec![
            PropertySet::new(
                "Pset_TransportElementCommon",
                common(vec![
                    p("CapacityPeople", m(Measure::Count, 8.0)),
                    p("CapacityWeight", m(Measure::Mass, 630.0)),
                ]),
            ),
            PropertySet::new(
                "Pset_TransportElementElevator",
                vec![p("ClearWidth", m(Measure::PositiveLength, 1.1))],
            ),
        ],
        vec![],
    );
    let mut gutter = element(0x110, ElementKind::PipeSegment, Some("GUTTER"));
    gutter.element_type = Some(0x201);
    add(
        gutter,
        vec![
            PropertySet::new(
                "Pset_PipeSegmentTypeGutter",
                vec![
                    p("Slope", m(Measure::PlaneAngle, 0.005)),
                    p("FlowRating", m(Measure::VolumetricFlowRate, 0.002)),
                ],
            ),
            PropertySet::new(
                "Pset_PipeSegmentTypeCommon",
                vec![
                    p("NominalDiameter", m(Measure::PositiveLength, 0.1)),
                    p("WorkingPressure", m(Measure::Pressure, 0.0)),
                ],
            ),
        ],
        vec![QuantitySet::new(
            "Qto_PipeSegmentBaseQuantities",
            vec![
                Quantity::new("Length", QuantityValue::Length(12.0)),
                Quantity::new("NetWeight", QuantityValue::Weight(18.0)),
            ],
        )],
    );
    let mut lift_type = ElementType::new(0x200, ElementKind::TransportElement, "Lift 630");
    lift_type.predefined_type = Some("ELEVATOR".into());
    let mut gutter_type = ElementType::new(0x201, ElementKind::PipeSegment, "Half-round 100");
    gutter_type.predefined_type = Some("GUTTER".into());
    model.types.extend([lift_type, gutter_type]);
    (model, sets)
}

fn single<'a>(file: &'a IfcFile, set: &str, property: &str) -> &'a Nominal {
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    let found = views
        .property_sets
        .iter()
        .filter(|s| s.definition.name.as_deref() == Some(set))
        .flat_map(|s| s.properties.iter())
        .find(|p| p.name.as_deref() == Some(property))
        .map(|p| p.value.clone())
        .unwrap();
    let ReadValue::Single {
        nominal: Some(nominal),
        ..
    } = found
    else {
        panic!("{set}.{property}: {found:?}");
    };
    // Leaked so the helper returns a reference like the views would.
    Box::leak(Box::new(nominal))
}

#[test]
fn standard_sets_read_back_and_match_their_templates() {
    let (model, sets) = standard_model();
    let (_, file) = written(&model, "write_values_standard.ifc");
    templates_match(&file, sets);
    // A plane angle is given in radians and written in the project's degrees.
    let pitch = single(&file, "Pset_SlabCommon", "PitchAngle");
    assert_eq!(pitch.type_name.as_deref(), Some("IFCPLANEANGLEMEASURE"));
    let Value::Real(degrees) = pitch.value else {
        panic!()
    };
    assert!((degrees - 30.0).abs() < 1e-9);
    let reference = single(&file, "Pset_WallCommon", "Reference");
    assert_eq!(reference.type_name.as_deref(), Some("IFCIDENTIFIER"));
    assert_eq!(
        single(&file, "Pset_StairCommon", "NumberOfRiser")
            .type_name
            .as_deref(),
        Some("IFCCOUNTMEASURE")
    );
    // One enumeration for every status value, referenced by each enumerated value.
    assert_eq!(file.instances_of("IFCPROPERTYENUMERATION").count(), 1);
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    let statuses: Vec<_> = views
        .property_sets
        .iter()
        .flat_map(|s| s.properties.iter())
        .filter(|p| p.name.as_deref() == Some("Status"))
        .collect();
    assert_eq!(statuses.len(), 14);
    for status in statuses {
        let ReadValue::Enumerated { values, reference } = &status.value else {
            panic!()
        };
        assert!(reference.is_some());
        assert_eq!(values[0].value, Value::Text("NEW".into()));
    }
    let weights: Vec<_> = views
        .quantity_sets
        .iter()
        .flat_map(|s| s.quantities.iter())
        .filter(|q| q.kind == QuantityKind::Weight)
        .collect();
    assert_eq!(weights.len(), 3);
}

#[test]
fn transport_elements_and_pipe_segments_read_back_with_their_types() {
    let (model, _) = standard_model();
    let (_, file) = written(&model, "write_values_kinds.ifc");
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    for (entity, kind, predefined, type_entity) in [
        (
            "IFCTRANSPORTELEMENT",
            ElementKind::TransportElement,
            "ELEVATOR",
            "IFCTRANSPORTELEMENTTYPE",
        ),
        (
            "IFCPIPESEGMENT",
            ElementKind::PipeSegment,
            "GUTTER",
            "IFCPIPESEGMENTTYPE",
        ),
    ] {
        let instance = file.instances_of(entity).next().unwrap();
        let record = &instance.records[0];
        assert_eq!(record.parameters.len(), 9, "{kind:?}");
        assert_eq!(
            record.parameters[8],
            Parameter::Enumeration(predefined.into())
        );
        let element_type = file.instances_of(type_entity).next().unwrap();
        assert_eq!(element_type.records[0].parameters.len(), 10);
        assert_eq!(
            element_type.records[0].parameters[9],
            Parameter::Enumeration(predefined.into())
        );
        assert!(
            views
                .types
                .iter()
                .any(|t| t.element.instance == instance.name
                    && t.element_type.instance == element_type.name)
        );
        let contained = views.spatial[0].children[0].children[0].children[0]
            .contained
            .iter()
            .any(|e| e.instance == instance.name);
        assert!(contained, "{kind:?} is contained in its storey");
    }
    // Every predefined value the writer knows is an item of the IFC4 enumeration.
    if let Some(tables) = SchemaTables::of(&Schema::Ifc4) {
        for (enumeration, values) in [
            (
                "IfcTransportElementTypeEnum",
                &[
                    "ELEVATOR",
                    "ESCALATOR",
                    "MOVINGWALKWAY",
                    "CRANEWAY",
                    "LIFTINGGEAR",
                ][..],
            ),
            (
                "IfcPipeSegmentTypeEnum",
                &[
                    "CULVERT",
                    "FLEXIBLESEGMENT",
                    "RIGIDSEGMENT",
                    "GUTTER",
                    "SPOOL",
                ][..],
            ),
        ] {
            let items = tables.enumeration(enumeration).unwrap();
            for value in values {
                assert!(items.contains(value), "{enumeration} {value}");
                let mut model = crate::model();
                let kind = if enumeration.contains("Transport") {
                    ElementKind::TransportElement
                } else {
                    ElementKind::PipeSegment
                };
                model.elements.push(element(0x100, kind, Some(value)));
                let bytes = write(&model, &WriteOptions::default()).unwrap().bytes;
                let text = String::from_utf8(bytes).unwrap();
                assert!(text.contains(&format!(",.{value}.);")), "{value}");
            }
        }
    }
}

#[test]
fn every_value_kind_is_written_with_its_type() {
    let mut model = model();
    let mut wall = element(0x100, ElementKind::Wall, None);
    let measures = [
        (Measure::Length, -1.0),
        (Measure::PositiveLength, 1.0),
        (Measure::NonNegativeLength, 0.0),
        (Measure::Area, 2.0),
        (Measure::Volume, 3.0),
        (Measure::PlaneAngle, PI),
        (Measure::PositivePlaneAngle, 0.5),
        (Measure::Ratio, -0.5),
        (Measure::PositiveRatio, 2.0),
        (Measure::NormalisedRatio, 1.0),
        (Measure::Count, 3.0),
        (Measure::ThermalTransmittance, 0.3),
        (Measure::ThermodynamicTemperature, 293.15),
        (Measure::Power, 100.0),
        (Measure::Pressure, 101_325.0),
        (Measure::VolumetricFlowRate, 0.01),
        (Measure::Mass, 10.0),
        (Measure::MassDensity, 2400.0),
        (Measure::AreaDensity, 50.0),
        (Measure::Time, 60.0),
        (Measure::Illuminance, 500.0),
        (Measure::LinearVelocity, 1.5),
        (Measure::HeatFluxDensity, 20.0),
        (Measure::PlanarForce, 3000.0),
        (Measure::Monetary, 99.5),
    ];
    let mut properties: Vec<Property> = measures
        .iter()
        .map(|(measure, value)| p(&format!("{measure:?}"), m(*measure, *value)))
        .collect();
    properties.extend([
        p("Identifier", PropertyValue::Identifier("ID-1".into())),
        p("Text", PropertyValue::Text("Long text".into())),
        p("Integer", PropertyValue::Integer(-7)),
        p("Real", PropertyValue::real(0.125).unwrap()),
        p("Logical", PropertyValue::Logical(None)),
        p("LogicalTrue", PropertyValue::Logical(Some(true))),
        p(
            "Choice",
            PropertyValue::enumerated("Arc_Choices", &["A", "B", "C"], &["A", "C"]).unwrap(),
        ),
    ]);
    wall.property_sets
        .push(PropertySet::new("ArcTest_Values", properties));
    wall.quantity_sets.push(QuantitySet::new(
        "ArcTest_Quantities",
        vec![
            Quantity::new("Count", QuantityValue::Count(4.0)),
            Quantity::new("Weight", QuantityValue::Weight(12.5)),
            Quantity::new("Time", QuantityValue::Time(3600.0)),
        ],
    ));
    model.elements.push(wall);
    let (bytes, file) = written(&model, "write_values_kinds_all.ifc");
    let text = String::from_utf8(bytes).unwrap();
    for (measure, _) in &measures {
        assert!(
            text.contains(&format!("{}(", measure.type_name())),
            "{measure:?}"
        );
    }
    for needle in [
        "IFCIDENTIFIER('ID-1')",
        "IFCTEXT('Long text')",
        "IFCINTEGER(-7)",
        "IFCREAL(0.125)",
        "IFCLOGICAL(.U.)",
        "IFCLOGICAL(.T.)",
        "IFCPROPERTYENUMERATION('Arc_Choices',(IFCLABEL('A'),IFCLABEL('B'),IFCLABEL('C')),$)",
        "IFCQUANTITYCOUNT('Count',$,$,4.,$)",
        "IFCQUANTITYWEIGHT('Weight',$,$,12.5,$)",
        "IFCQUANTITYTIME('Time',$,$,3600.,$)",
    ] {
        assert!(text.contains(needle), "{needle}");
    }
    // Radians in a radian project are written unchanged.
    assert!(text.contains(&format!("IFCPLANEANGLEMEASURE({PI})")));
    let views = file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    let kinds: Vec<_> = views.quantity_sets[0]
        .quantities
        .iter()
        .map(|q| q.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            QuantityKind::Count,
            QuantityKind::Weight,
            QuantityKind::Time
        ]
    );
    assert_eq!(views.property_sets[0].properties.len(), measures.len() + 7);
    // Non-standard names are not compared with templates.
    templates_match(&file, 0);
}

#[test]
fn invalid_values_are_refused() {
    let refused = |result: arc_base::Result<PropertyValue>| {
        assert_eq!(
            result.unwrap_err().code(),
            codes::IFC_WRITE_VALUE_INVALID.code
        );
    };
    refused(PropertyValue::measure(Measure::PositiveLength, 0.0));
    refused(PropertyValue::measure(Measure::PositiveLength, -1.0));
    refused(PropertyValue::measure(Measure::NonNegativeLength, -1e-9));
    refused(PropertyValue::measure(Measure::NormalisedRatio, 1.5));
    refused(PropertyValue::measure(Measure::PositiveRatio, 0.0));
    refused(PropertyValue::measure(Measure::PositivePlaneAngle, 0.0));
    refused(PropertyValue::measure(Measure::Length, f64::NAN));
    refused(PropertyValue::measure(Measure::Power, f64::INFINITY));
    refused(PropertyValue::real(f64::NAN));
    refused(PropertyValue::label(&"x".repeat(256)));
    refused(PropertyValue::identifier("a\0b"));
    refused(PropertyValue::enumerated("E", &["A"], &["B"]));
    refused(PropertyValue::enumerated("E", &["A"], &[]));
    refused(PropertyValue::enumerated("E", &[], &[]));
    refused(PropertyValue::enumerated("E", &["A", "A"], &["A"]));
    assert!(PropertyValue::label(&"ğ".repeat(255)).is_ok());

    // Values built directly are checked when written.
    let write_with = |properties: Vec<Property>, quantities: Vec<Quantity>| {
        let mut model = model();
        let mut wall = element(0x100, ElementKind::Wall, None);
        if !properties.is_empty() {
            wall.property_sets.push(PropertySet::new("P", properties));
        }
        if !quantities.is_empty() {
            wall.quantity_sets.push(QuantitySet::new("Q", quantities));
        }
        model.elements.push(wall);
        write(&model, &WriteOptions::default())
    };
    let refused_write = |properties, quantities| {
        let error = write_with(properties, quantities).unwrap_err();
        assert_eq!(error.code(), codes::IFC_WRITE_VALUE_INVALID.code);
    };
    refused_write(
        vec![p(
            "A",
            PropertyValue::Measure(Measure::PositiveLength, -2.0),
        )],
        vec![],
    );
    refused_write(vec![p("A", PropertyValue::Real(f64::NAN))], vec![]);
    refused_write(
        vec![p(
            "A",
            PropertyValue::Enumerated {
                enumeration: "E".into(),
                items: vec!["A".into()],
                values: vec!["Z".into()],
            },
        )],
        vec![],
    );
    // One enumeration name with two item lists.
    refused_write(
        vec![
            p("A", PropertyValue::enumerated("E", &["A"], &["A"]).unwrap()),
            p(
                "B",
                PropertyValue::enumerated("E", &["A", "B"], &["B"]).unwrap(),
            ),
        ],
        vec![],
    );
    for quantity in [
        QuantityValue::Count(-1.0),
        QuantityValue::Weight(f64::NAN),
        QuantityValue::Time(-0.5),
    ] {
        refused_write(vec![], vec![Quantity::new("X", quantity)]);
    }
    // The same enumeration with the same items is written once.
    let output = write_with(
        vec![
            p(
                "A",
                PropertyValue::enumerated("E", &["A", "B"], &["A"]).unwrap(),
            ),
            p(
                "B",
                PropertyValue::enumerated("E", &["A", "B"], &["B"]).unwrap(),
            ),
        ],
        vec![],
    )
    .unwrap();
    let text = String::from_utf8(output.bytes).unwrap();
    assert_eq!(text.matches("IFCPROPERTYENUMERATION(").count(), 1);
}

fn grid_model(axes_u: Vec<GridAxis>) -> WriteModel {
    let mut model = model();
    model.grids.push(Grid::new(
        0x300,
        "Grid",
        STOREY,
        axes_u,
        vec![GridAxis::new(
            "1",
            AxisCurve::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, 0.0),
            },
        )],
    ));
    model
}

fn pt(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

#[test]
fn polyline_and_composite_axes_are_written() {
    let polyline = AxisCurve::Polyline(vec![pt(0.0, 0.0), pt(0.0, 5.0), pt(2.0, 9.0)]);
    // A line, a quarter arc clockwise (a reversed counterclockwise arc) and a line.
    let open = AxisCurve::Composite(vec![
        AxisSegment::new(AxisCurve::Line {
            start: pt(4.0, 0.0),
            end: pt(4.0, 5.0),
        }),
        AxisSegment::reversed(AxisCurve::Arc {
            center: pt(6.0, 5.0),
            radius: 2.0,
            start_angle: FRAC_PI_2,
            end_angle: PI,
        }),
        AxisSegment::new(AxisCurve::Polyline(vec![pt(6.0, 7.0), pt(9.0, 7.0)])),
    ]);
    // A closed curve: a half circle and its diameter.
    let closed = AxisCurve::Composite(vec![
        AxisSegment::new(AxisCurve::Arc {
            center: pt(0.0, 20.0),
            radius: 3.0,
            start_angle: 0.0,
            end_angle: PI,
        }),
        AxisSegment::new(AxisCurve::Line {
            start: pt(-3.0, 20.0),
            end: pt(3.0, 20.0),
        }),
    ]);
    let model = grid_model(vec![
        GridAxis::new("A", polyline),
        GridAxis::new("B", open),
        GridAxis::new("C", closed),
    ]);
    let (bytes, file) = written(&model, "write_values_axes.ifc");
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(file.instances_of("IFCCOMPOSITECURVE").count(), 2);
    assert_eq!(file.instances_of("IFCCOMPOSITECURVESEGMENT").count(), 5);
    assert_eq!(text.matches(".DISCONTINUOUS.").count(), 1);
    assert!(text.contains("IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.F.,"));
    let three = file
        .instances_of("IFCPOLYLINE")
        .filter(|i| matches!(&i.records[0].parameters[0], Parameter::List(l) if l.len() == 3))
        .count();
    assert_eq!(three, 1);

    for bad in [
        AxisCurve::Polyline(vec![pt(0.0, 0.0)]),
        AxisCurve::Polyline(vec![pt(0.0, 0.0), pt(0.0, 0.0), pt(1.0, 0.0)]),
        AxisCurve::Polyline(vec![pt(0.0, 0.0), pt(f64::NAN, 1.0)]),
        AxisCurve::Composite(vec![]),
        // A gap between the segments.
        AxisCurve::Composite(vec![
            AxisSegment::new(AxisCurve::Line {
                start: pt(0.0, 0.0),
                end: pt(1.0, 0.0),
            }),
            AxisSegment::new(AxisCurve::Line {
                start: pt(1.5, 0.0),
                end: pt(2.0, 0.0),
            }),
        ]),
        // A composite inside a composite.
        AxisCurve::Composite(vec![AxisSegment::new(AxisCurve::Composite(vec![]))]),
    ] {
        let error = write(
            &grid_model(vec![GridAxis::new("X", bad)]),
            &WriteOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), codes::IFC_WRITE_VALUE_INVALID.code);
    }
}

#[test]
fn groups_assign_elements_and_grids() {
    let mut model = grid_model(vec![GridAxis::new(
        "A",
        AxisCurve::Line {
            start: pt(0.0, 0.0),
            end: pt(0.0, 10.0),
        },
    )]);
    model.elements.push(element(0x100, ElementKind::Wall, None));
    model.elements.push(element(0x101, ElementKind::Slab, None));
    let mut group = Group::new(0x400, "Selection 1", vec![0x100, 0x300, 0x100, 0x101]);
    group.description = Some("Walls and grid".into());
    group.object_type = Some("Selection".into());
    model.groups.push(group);
    model.groups.push(Group::new(0x401, "Empty", vec![]));
    let (bytes, file) = written(&model, "write_values_groups.ifc");
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(file.instances_of("IFCGROUP").count(), 2);
    let rels: Vec<_> = file.instances_of("IFCRELASSIGNSTOGROUP").collect();
    assert_eq!(rels.len(), 1);
    let Parameter::List(members) = &rels[0].records[0].parameters[4] else {
        panic!()
    };
    assert_eq!(members.len(), 3, "a repeated member is written once");
    assert!(text.contains("'Selection 1','Walls and grid','Selection');"));

    let mut bad = model.clone();
    bad.groups = vec![Group::new(0x402, "Missing", vec![0x999])];
    assert_eq!(
        write(&bad, &WriteOptions::default()).unwrap_err().code(),
        codes::IFC_WRITE_REFERENCE_INVALID.code
    );
    let mut duplicate = model;
    duplicate.groups = vec![Group::new(0x100, "Same id as a wall", vec![])];
    assert_eq!(
        write(&duplicate, &WriteOptions::default())
            .unwrap_err()
            .code(),
        codes::IFC_WRITE_ID_DUPLICATE.code
    );
}

#[test]
fn georeferencing_writes_a_map_conversion() {
    let mut model = model();
    let mut georeference = Georeference::new("EPSG:32635", 500_000.0, 4_540_000.0, 35.5);
    georeference.geodetic_datum = Some("WGS84".into());
    georeference.map_projection = Some("UTM".into());
    georeference.map_zone = Some("35N".into());
    georeference.x_axis = Some(Vector2::new(0.0, 2.0));
    georeference.scale = Some(0.9996);
    model.georeference = Some(georeference.clone());
    let (bytes, file) = written(&model, "write_values_georeference.ifc");
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("IFCPROJECTEDCRS('EPSG:32635',$,'WGS84',$,'UTM','35N',#"));
    let conversion = file.instances_of("IFCMAPCONVERSION").next().unwrap();
    let parameters = &conversion.records[0].parameters;
    let Parameter::Reference(source) = parameters[0] else {
        panic!()
    };
    assert_eq!(
        file.instance(source).unwrap().records[0].type_name,
        "IFCGEOMETRICREPRESENTATIONCONTEXT"
    );
    assert_eq!(
        parameters[2..],
        [
            Parameter::Real(500_000.0),
            Parameter::Real(4_540_000.0),
            Parameter::Real(35.5),
            Parameter::Real(0.0),
            Parameter::Real(1.0),
            Parameter::Real(0.9996),
        ]
    );
    for change in [
        |g: &mut Georeference| g.scale = Some(0.0),
        |g: &mut Georeference| g.x_axis = Some(Vector2::new(0.0, 0.0)),
        |g: &mut Georeference| g.eastings = f64::NAN,
        |g: &mut Georeference| g.crs_name = String::new(),
        |g: &mut Georeference| g.map_zone = Some("z".repeat(256)),
    ] {
        let mut bad = georeference.clone();
        change(&mut bad);
        model.georeference = Some(bad);
        assert_eq!(
            write(&model, &WriteOptions::default()).unwrap_err().code(),
            codes::IFC_WRITE_VALUE_INVALID.code
        );
    }
}

#[test]
fn a_set_on_another_predefined_type_or_entity_is_reported() {
    let mut model = model();
    let mut pipe = element(0x100, ElementKind::PipeSegment, Some("RIGIDSEGMENT"));
    pipe.property_sets.push(PropertySet::new(
        "Pset_PipeSegmentTypeGutter",
        vec![p("Slope", m(Measure::PlaneAngle, 0.01))],
    ));
    let mut slab = element(0x101, ElementKind::Slab, None);
    slab.property_sets.push(PropertySet::new(
        "Pset_WallCommon",
        vec![p("IsExternal", PropertyValue::Boolean(true))],
    ));
    model.elements.extend([pipe, slab]);
    let (_, file) = written(&model, "write_values_not_applicable.ifc");
    let check = file
        .check_property_sets(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    if check.templates.is_none() || SchemaTables::of(&Schema::Ifc4).is_none() {
        return;
    }
    let found: Vec<_> = check
        .findings
        .iter()
        .map(|f| (f.code.code, f.found.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            ("io.ifc.pset_not_applicable", "IFCPIPESEGMENT/RIGIDSEGMENT"),
            ("io.ifc.pset_not_applicable", "IFCSLAB"),
        ]
    );
}
