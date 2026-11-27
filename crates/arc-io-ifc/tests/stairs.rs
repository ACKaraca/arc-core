//! IFC writing of stairs, flights, landings and railings (CORE-IO-016): attributes, aggregation
//! of the parts, containment of the rest, and a read-back of the geometry.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

use arc_base::math::{Point3, Vector3};
use arc_base::{Cancel, Limits};
use arc_io_ifc::write::{
    Body, Element, ElementKind, FlightDimensions, Placement, Quantity, QuantitySet, QuantityValue,
    Spatial, Storey,
};
use arc_io_ifc::{
    IfcFile, Parameter, ReadOptions, WriteModel, WriteOptions, global_id, read, write,
};
use arc_mesh::measure::volume;

const STOREY: u128 = 0x100;
const STRAIGHT: u128 = 0x1000;
const L_STAIR: u128 = 0x2000;
const U_STAIR: u128 = 0x3000;
const GUARD: u128 = 0x4000;

const RISERS: u32 = 5;
const RISER: f64 = 0.18;
const TREAD: f64 = 0.28;
const WIDTH: f64 = 1.0;

/// A flight as a faceted B-rep: the stepped side profile (x, z) extruded by `WIDTH` along y.
fn flight_body() -> Body {
    let (n, h, t) = (f64::from(RISERS), RISER, TREAD);
    let mut profile = vec![(0.0, 0.0), (n * t, 0.0), (n * t, n * h)];
    for i in (0..RISERS).rev() {
        let i = f64::from(i);
        profile.push((i * t, (i + 1.0) * h));
        if i > 0.0 {
            profile.push((i * t, i * h));
        }
    }
    let count = profile.len();
    let vertices = profile
        .iter()
        .map(|&(x, z)| Point3::new(x, 0.0, z))
        .chain(profile.iter().map(|&(x, z)| Point3::new(x, WIDTH, z)))
        .collect();
    let mut faces = vec![
        (0..count).collect::<Vec<_>>(),
        (count..2 * count).rev().collect(),
    ];
    faces.extend((0..count).map(|a| {
        let b = (a + 1) % count;
        vec![a, a + count, b + count, b]
    }));
    Body::Brep { vertices, faces }
}

fn flight(id: u128, name: &str, x: f64) -> Element {
    let mut flight = part(id, ElementKind::StairFlight, name, id & !0xfff);
    flight.placement = Placement::new(Point3::new(x, 0.0, 0.0), Vector3::z(), Vector3::x());
    flight.body = Some(flight_body());
    flight.predefined_type = Some("straight".into());
    flight.flight = Some(FlightDimensions::new(RISERS, RISERS - 1, RISER, TREAD));
    flight.quantity_sets.push(QuantitySet::new(
        "Qto_StairFlightBaseQuantities",
        vec![Quantity::new(
            "Length",
            QuantityValue::Length(f64::from(RISERS) * TREAD),
        )],
    ));
    flight
}

fn part(id: u128, kind: ElementKind, name: &str, host: u128) -> Element {
    let mut part = Element::new(id, kind, name, STOREY);
    part.host = Some(host);
    part
}

fn model() -> WriteModel {
    let mut model = WriteModel::new(
        Spatial::new(1, "Project"),
        Spatial::new(2, "Site"),
        Spatial::new(3, "Building"),
    );
    model.storeys.push(Storey::new(STOREY, "Ground", 0.0));
    let stair = |id, name: &str, predefined: &str| {
        let mut stair = Element::new(id, ElementKind::Stair, name, STOREY);
        stair.predefined_type = Some(predefined.into());
        stair
    };
    let landing = |id, host| {
        let mut landing = part(id, ElementKind::Slab, "Landing", host);
        landing.predefined_type = Some("landing".into());
        landing.body = Some(Body::Extrusion {
            outer: vec![
                arc_base::math::Point2::new(0.0, 0.0),
                arc_base::math::Point2::new(1.0, 0.0),
                arc_base::math::Point2::new(1.0, 1.0),
                arc_base::math::Point2::new(0.0, 1.0),
            ],
            holes: Vec::new(),
            direction: Vector3::z(),
            depth: 0.15,
        });
        landing
    };
    let railing = |id, name: &str, kind: &str, host| {
        let mut railing = Element::new(id, ElementKind::Railing, name, STOREY);
        railing.predefined_type = Some(kind.into());
        railing.host = host;
        railing.quantity_sets.push(QuantitySet::new(
            "Qto_RailingBaseQuantities",
            vec![Quantity::new("Length", QuantityValue::Length(1.4))],
        ));
        railing
    };
    model.elements.extend([
        stair(STRAIGHT, "Straight", "straight_run_stair"),
        flight(STRAIGHT + 1, "Flight", 0.0),
        railing(STRAIGHT + 2, "Handrail", "handrail", Some(STRAIGHT)),
        stair(L_STAIR, "L", "quarter_turn_stair"),
        flight(L_STAIR + 1, "Flight 1", 5.0),
        landing(L_STAIR + 2, L_STAIR),
        flight(L_STAIR + 3, "Flight 2", 7.0),
        stair(U_STAIR, "U", "HALF_TURN_STAIR"),
        flight(U_STAIR + 1, "Flight 1", 10.0),
        landing(U_STAIR + 2, U_STAIR),
        flight(U_STAIR + 3, "Flight 2", 12.0),
        railing(GUARD, "Guardrail", "guardrail", None),
    ]);
    model
}

fn options() -> WriteOptions {
    WriteOptions {
        timestamp: "2026-01-02T03:04:05".into(),
        ..WriteOptions::default()
    }
}

fn name_of(file: &IfcFile, id: u128) -> u64 {
    let wanted = global_id(id);
    file.instances()
        .iter()
        .find(
            |i| matches!(i.records[0].parameters.first(), Some(Parameter::Text(t)) if *t == wanted),
        )
        .map(|i| i.name)
        .expect("instance")
}

fn references(parameter: &Parameter) -> Vec<u64> {
    match parameter {
        Parameter::List(items) => items
            .iter()
            .map(|i| match i {
                Parameter::Reference(r) => *r,
                other => panic!("{other:?}"),
            })
            .collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn stairs_parts_and_railings_read_back() {
    let model = model();
    let output = write(&model, &options()).unwrap();
    assert_eq!(output.bytes, write(&model, &options()).unwrap().bytes);
    assert!(!output.report.has_loss(), "{:?}", output.report);
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    let record = |id| &file.instance(name_of(&file, id)).unwrap().records[0];

    // IfcStair: Tag 7, PredefinedType 8.
    for (id, expected) in [
        (STRAIGHT, "STRAIGHT_RUN_STAIR"),
        (L_STAIR, "QUARTER_TURN_STAIR"),
        (U_STAIR, "HALF_TURN_STAIR"),
    ] {
        assert_eq!(record(id).type_name, "IFCSTAIR");
        assert_eq!(record(id).parameters.len(), 9);
        assert_eq!(
            record(id).parameters[8],
            Parameter::Enumeration(expected.into())
        );
    }
    // IfcStairFlight: NumberOfRisers 8, NumberOfTreads 9, RiserHeight 10, TreadLength 11,
    // PredefinedType 12.
    let flight = record(STRAIGHT + 1);
    assert_eq!(flight.type_name, "IFCSTAIRFLIGHT");
    assert_eq!(flight.parameters[8], Parameter::Integer(5));
    assert_eq!(flight.parameters[9], Parameter::Integer(4));
    assert_eq!(flight.parameters[10], Parameter::Real(0.18));
    assert_eq!(flight.parameters[11], Parameter::Real(0.28));
    assert_eq!(
        flight.parameters[12],
        Parameter::Enumeration("STRAIGHT".into())
    );
    // IfcRailing: PredefinedType 8.
    assert_eq!(record(STRAIGHT + 2).type_name, "IFCRAILING");
    assert_eq!(
        record(STRAIGHT + 2).parameters[8],
        Parameter::Enumeration("HANDRAIL".into())
    );
    assert_eq!(record(L_STAIR + 2).type_name, "IFCSLAB");
    assert_eq!(
        record(L_STAIR + 2).parameters[8],
        Parameter::Enumeration("LANDING".into())
    );

    // Each stair aggregates its parts in model order.
    let aggregates: Vec<_> = file
        .instances_of("IFCRELAGGREGATES")
        .map(|i| &i.records[0].parameters)
        .collect();
    let parts_of = |stair: u128| {
        let owner = name_of(&file, stair);
        let found: Vec<_> = aggregates
            .iter()
            .filter(|p| p[4] == Parameter::Reference(owner))
            .collect();
        assert_eq!(found.len(), 1);
        references(&found[0][5])
    };
    let names = |ids: &[u128]| ids.iter().map(|&i| name_of(&file, i)).collect::<Vec<_>>();
    assert_eq!(parts_of(STRAIGHT), names(&[STRAIGHT + 1, STRAIGHT + 2]));
    assert_eq!(
        parts_of(L_STAIR),
        names(&[L_STAIR + 1, L_STAIR + 2, L_STAIR + 3])
    );
    assert_eq!(
        parts_of(U_STAIR),
        names(&[U_STAIR + 1, U_STAIR + 2, U_STAIR + 3])
    );

    // The storey contains the stairs and the free railing, not the parts.
    let contained: Vec<_> = file
        .instances_of("IFCRELCONTAINEDINSPATIALSTRUCTURE")
        .collect();
    assert_eq!(contained.len(), 1);
    assert_eq!(
        references(&contained[0].records[0].parameters[4]),
        names(&[STRAIGHT, L_STAIR, U_STAIR, GUARD])
    );

    // The quantity sets are attached, and the flights have the volume of their steps.
    assert_eq!(file.instances_of("IFCELEMENTQUANTITY").count(), 7);
    let geometry = file.geometry(&Limits::CONSTRAINED, &Cancel::new()).unwrap();
    let flight_name = name_of(&file, STRAIGHT + 1);
    let item = geometry
        .items
        .iter()
        .find(|i| i.product == flight_name)
        .unwrap();
    let mesh = item.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap();
    let steps = f64::from(RISERS * (RISERS + 1) / 2);
    let expected = WIDTH * TREAD * RISER * steps;
    assert!((volume(&mesh, &Limits::CONSTRAINED).unwrap() - expected).abs() < 1e-9);
}

/// The aggregation of parts is grouped by host in one pass; the bytes stay what the
/// container-by-element scan wrote.
#[test]
fn stair_output_bytes_are_pinned() {
    let bytes = write(&model(), &options()).unwrap().bytes;
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    assert_eq!(hash, 3_995_293_198_085_456_176, "{} bytes", bytes.len());
}

#[test]
fn invalid_stair_parts_are_refused() {
    let code = |model: &WriteModel| write(model, &options()).unwrap_err().code();
    // A flight is a part of a stair only.
    let mut bad = model();
    bad.elements[1].host = Some(GUARD);
    assert_eq!(code(&bad), "io.ifc.write_reference_invalid");
    // A slab without a stair has no host, a stair has none either.
    let mut bad = model();
    bad.elements[0].host = Some(L_STAIR);
    assert_eq!(code(&bad), "io.ifc.write_reference_invalid");
    let mut bad = model();
    bad.elements[1].flight = Some(FlightDimensions::new(5, 4, 0.0, TREAD));
    assert_eq!(code(&bad), "io.ifc.write_value_invalid");
}
