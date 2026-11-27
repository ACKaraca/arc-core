//! IFC writing of ramps, curtain walls with plates and members, and grids (CORE-IO-016):
//! attributes, aggregation of the parts, containment, grid axes and refusals.
//!
//! `ARC_IFC_DUMP=<directory>` writes the produced files there, for the independent oracle.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

mod support;

use arc_base::math::{Point2, Point3, Vector3};
use arc_io_ifc::write::{
    AngleUnit, AxisCurve, Body, Element, ElementKind, ElementType, Grid, GridAxis, Placement,
    Quantity, QuantitySet, QuantityValue, Spatial, Storey,
};
use arc_io_ifc::{
    IfcFile, Parameter, ReadOptions, WriteModel, WriteOptions, global_id, read, write,
};

const STOREY: u128 = 0x100;
const RAMP: u128 = 0x1000;
const WALL: u128 = 0x2000;
const GRID: u128 = 0x3000;
const PANEL_TYPE: u128 = 0x4000;

fn slab_body(size: f64, depth: f64) -> Body {
    Body::Extrusion {
        outer: vec![
            Point2::new(0.0, 0.0),
            Point2::new(size, 0.0),
            Point2::new(size, size),
            Point2::new(0.0, size),
        ],
        holes: Vec::new(),
        direction: Vector3::z(),
        depth,
    }
}

fn part(id: u128, kind: ElementKind, name: &str, host: u128, predefined: &str) -> Element {
    let mut part = Element::new(id, kind, name, STOREY);
    part.host = Some(host);
    part.predefined_type = Some(predefined.into());
    part.body = Some(slab_body(1.0, 0.1));
    part
}

fn line(a: (f64, f64), b: (f64, f64)) -> AxisCurve {
    AxisCurve::Line {
        start: Point2::new(a.0, a.1),
        end: Point2::new(b.0, b.1),
    }
}

fn model() -> WriteModel {
    let mut model = WriteModel::new(
        Spatial::new(1, "Project"),
        Spatial::new(2, "Site"),
        Spatial::new(3, "Building"),
    );
    model.storeys.push(Storey::new(STOREY, "Ground", 0.0));
    let mut ramp = Element::new(RAMP, ElementKind::Ramp, "Ramp", STOREY);
    ramp.predefined_type = Some("quarter_turn_ramp".into());
    let mut flight = part(RAMP + 1, ElementKind::RampFlight, "Run", RAMP, "straight");
    flight.quantity_sets.push(QuantitySet::new(
        "Qto_RampFlightBaseQuantities",
        vec![Quantity::new("Length", QuantityValue::Length(6.0))],
    ));
    let landing = part(RAMP + 2, ElementKind::Slab, "Landing", RAMP, "landing");
    let mut rail = Element::new(RAMP + 3, ElementKind::Railing, "Rail", STOREY);
    rail.host = Some(RAMP);
    rail.predefined_type = Some("handrail".into());
    let mut free = Element::new(RAMP + 4, ElementKind::RampFlight, "Free run", STOREY);
    free.predefined_type = Some("spiral".into());

    let mut wall = Element::new(WALL, ElementKind::CurtainWall, "Facade", STOREY);
    wall.predefined_type = Some("userdefined".into());
    let mut panel = part(WALL + 1, ElementKind::Plate, "Panel", WALL, "curtain_panel");
    panel.element_type = Some(PANEL_TYPE);
    let panel2 = part(
        WALL + 2,
        ElementKind::Plate,
        "Panel 2",
        WALL,
        "curtain_panel",
    );
    let mullion = part(WALL + 3, ElementKind::Member, "Mullion", WALL, "mullion");
    let transom = part(WALL + 4, ElementKind::Member, "Transom", WALL, "transom");
    model.elements.extend([
        ramp, flight, landing, rail, free, wall, panel, panel2, mullion, transom,
    ]);
    model.types.push(ElementType::new(
        PANEL_TYPE,
        ElementKind::Plate,
        "Glass panel",
    ));

    let mut u = GridAxis::new("A", line((0.0, 0.0), (0.0, 10.0)));
    u.same_sense = false;
    let arc = AxisCurve::Arc {
        center: Point2::new(0.0, 0.0),
        radius: 5.0,
        start_angle: 0.0,
        end_angle: std::f64::consts::FRAC_PI_2,
    };
    let mut grid = Grid::new(
        GRID,
        "Grid",
        STOREY,
        vec![u, GridAxis::new("B", line((5.0, 0.0), (5.0, 10.0)))],
        vec![
            GridAxis::new("1", line((0.0, 0.0), (10.0, 0.0))),
            GridAxis::new("2", arc),
        ],
    );
    grid.predefined_type = Some("irregular".into());
    grid.placement = Placement::new(Point3::new(1.0, 2.0, 0.0), Vector3::z(), Vector3::x());
    model.grids.push(grid);
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

fn en(text: &str) -> Parameter {
    Parameter::Enumeration(text.into())
}

#[test]
fn ramps_curtain_walls_and_grids_read_back() {
    let model = model();
    let output = write(&model, &options()).unwrap();
    support::dump("ramps-grids.ifc", &output.bytes);
    assert_eq!(output.bytes, write(&model, &options()).unwrap().bytes);
    assert!(!output.report.has_loss(), "{:?}", output.report);
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    let record = |id| &file.instance(name_of(&file, id)).unwrap().records[0];
    let names = |ids: &[u128]| ids.iter().map(|&i| name_of(&file, i)).collect::<Vec<_>>();

    // Tag 7, PredefinedType 8 for each of these entities.
    for (id, entity, predefined) in [
        (RAMP, "IFCRAMP", "QUARTER_TURN_RAMP"),
        (RAMP + 1, "IFCRAMPFLIGHT", "STRAIGHT"),
        (RAMP + 3, "IFCRAILING", "HANDRAIL"),
        (RAMP + 4, "IFCRAMPFLIGHT", "SPIRAL"),
        (WALL, "IFCCURTAINWALL", "USERDEFINED"),
        (WALL + 1, "IFCPLATE", "CURTAIN_PANEL"),
        (WALL + 3, "IFCMEMBER", "MULLION"),
        (WALL + 4, "IFCMEMBER", "USERDEFINED"),
    ] {
        assert_eq!(record(id).type_name, entity);
        assert_eq!(record(id).parameters.len(), 9, "{entity}");
        assert_eq!(record(id).parameters[8], en(predefined), "{entity}");
    }
    assert_eq!(
        record(WALL + 4).parameters[4],
        Parameter::Text("transom".into())
    );
    assert_eq!(record(RAMP + 2).parameters[8], en("LANDING"));

    // Parts are aggregated by their ramp or curtain wall, in model order.
    let aggregates: Vec<_> = file
        .instances_of("IFCRELAGGREGATES")
        .map(|i| &i.records[0].parameters)
        .collect();
    let parts_of = |owner: u128| {
        let owner = name_of(&file, owner);
        let found: Vec<_> = aggregates
            .iter()
            .filter(|p| p[4] == Parameter::Reference(owner))
            .collect();
        assert_eq!(found.len(), 1);
        references(&found[0][5])
    };
    assert_eq!(parts_of(RAMP), names(&[RAMP + 1, RAMP + 2, RAMP + 3]));
    assert_eq!(
        parts_of(WALL),
        names(&[WALL + 1, WALL + 2, WALL + 3, WALL + 4])
    );

    // The storey contains the ramp, the free flight, the curtain wall and the grid.
    let contained: Vec<_> = file
        .instances_of("IFCRELCONTAINEDINSPATIALSTRUCTURE")
        .collect();
    assert_eq!(contained.len(), 1);
    assert_eq!(
        references(&contained[0].records[0].parameters[4]),
        names(&[RAMP, RAMP + 4, WALL, GRID])
    );

    // The plate type and its occurrence.
    let plate_type = file.instances_of("IFCPLATETYPE").next().unwrap();
    assert_eq!(
        plate_type.records[0].parameters[9],
        Parameter::Enumeration("NOTDEFINED".into())
    );
    let typed = file.instances_of("IFCRELDEFINESBYTYPE").next().unwrap();
    assert_eq!(
        references(&typed.records[0].parameters[4]),
        names(&[WALL + 1])
    );

    // IfcGrid: ObjectType 4, UAxes 7, VAxes 8, WAxes 9, PredefinedType 10 (zero-based 7..=10).
    let grid = record(GRID);
    assert_eq!(grid.type_name, "IFCGRID");
    assert_eq!(grid.parameters.len(), 11);
    assert_eq!(grid.parameters[9], Parameter::Omitted);
    assert_eq!(grid.parameters[10], en("IRREGULAR"));
    let axes = |parameter: &Parameter| -> Vec<(String, Parameter, Parameter)> {
        references(parameter)
            .into_iter()
            .map(|n| {
                let p = &file.instance(n).unwrap().records[0];
                assert_eq!(p.type_name, "IFCGRIDAXIS");
                let Parameter::Text(tag) = p.parameters[0].clone() else {
                    panic!()
                };
                (tag, p.parameters[1].clone(), p.parameters[2].clone())
            })
            .collect()
    };
    let u = axes(&grid.parameters[7]);
    let v = axes(&grid.parameters[8]);
    assert_eq!(
        u.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(
        v.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert_eq!(u[0].2, en("F"));
    assert_eq!(u[1].2, en("T"));
    let curve = |axis: &(String, Parameter, Parameter)| match axis.1 {
        Parameter::Reference(n) => &file.instance(n).unwrap().records[0],
        ref other => panic!("{other:?}"),
    };
    assert_eq!(curve(&u[0]).type_name, "IFCPOLYLINE");
    assert_eq!(references(&curve(&u[0]).parameters[0]).len(), 2);
    let arc = curve(&v[1]);
    assert_eq!(arc.type_name, "IFCTRIMMEDCURVE");
    assert_eq!(arc.parameters[3], en("T"));
    assert_eq!(arc.parameters[4], en("PARAMETER"));
    let Parameter::List(last) = &arc.parameters[2] else {
        panic!()
    };
    assert_eq!(
        last[0],
        Parameter::Typed {
            type_name: "IFCPARAMETERVALUE".into(),
            value: Box::new(Parameter::Real(std::f64::consts::FRAC_PI_2)),
        }
    );
    let Parameter::Reference(circle) = arc.parameters[0] else {
        panic!()
    };
    assert_eq!(
        file.instance(circle).unwrap().records[0].type_name,
        "IFCCIRCLE"
    );

    // The representation is a FootPrint GeometricCurveSet of the four axis curves.
    let Parameter::Reference(shape) = grid.parameters[6] else {
        panic!()
    };
    let shape = &file.instance(shape).unwrap().records[0];
    let representation = references(&shape.parameters[2])[0];
    let representation = &file.instance(representation).unwrap().records[0];
    assert_eq!(
        representation.parameters[1],
        Parameter::Text("FootPrint".into())
    );
    assert_eq!(
        representation.parameters[2],
        Parameter::Text("GeometricCurveSet".into())
    );
    let set = references(&representation.parameters[3])[0];
    let set = &file.instance(set).unwrap().records[0];
    assert_eq!(set.type_name, "IFCGEOMETRICCURVESET");
    assert_eq!(references(&set.parameters[0]).len(), 4);
}

#[test]
fn arc_angles_follow_the_angle_unit() {
    let mut model = model();
    model.angle_unit = AngleUnit::Degree;
    let output = write(&model, &options()).unwrap();
    support::dump("ramps-degrees.ifc", &output.bytes);
    let text = String::from_utf8(output.bytes).unwrap();
    assert!(text.contains("IFCPARAMETERVALUE(90.)"), "{text}");
}

#[test]
fn invalid_parts_and_grids_are_refused() {
    let code = |model: &WriteModel| write(model, &options()).unwrap_err().code();
    let (reference, value) = (
        "io.ifc.write_reference_invalid",
        "io.ifc.write_value_invalid",
    );
    // A ramp flight is a part of a ramp only; a plate and a member of a curtain wall only.
    let mut bad = model();
    bad.elements[1].host = Some(WALL);
    assert_eq!(code(&bad), reference);
    let mut bad = model();
    bad.elements[6].host = Some(RAMP);
    assert_eq!(code(&bad), reference);
    let mut bad = model();
    bad.elements[8].host = Some(RAMP + 1);
    assert_eq!(code(&bad), reference);
    let mut bad = model();
    bad.elements[0].host = Some(WALL);
    assert_eq!(code(&bad), reference);
    // A grid needs its storey, a U and a V axis, and valid curves.
    let mut bad = model();
    bad.grids[0].storey = 0xdead;
    assert_eq!(code(&bad), reference);
    let mut bad = model();
    bad.grids[0].axes_v.clear();
    assert_eq!(code(&bad), value);
    let mut bad = model();
    bad.grids[0].axes_u[0].curve = line((1.0, 1.0), (1.0, 1.0));
    assert_eq!(code(&bad), value);
    let mut bad = model();
    bad.grids[0].axes_u[0].curve = line((f64::NAN, 1.0), (1.0, 1.0));
    assert_eq!(code(&bad), value);
    for (radius, start, end) in [
        (0.0, 0.0, 1.0),
        (1.0, 1.0, 1.0),
        (1.0, 0.0, 7.0),
        (1.0, 0.0, f64::NAN),
    ] {
        let mut bad = model();
        bad.grids[0].axes_v[1].curve = AxisCurve::Arc {
            center: Point2::new(0.0, 0.0),
            radius,
            start_angle: start,
            end_angle: end,
        };
        assert_eq!(code(&bad), value, "{radius} {start} {end}");
    }
    // A duplicate id is refused.
    let mut bad = model();
    bad.grids.push(bad.grids[0].clone());
    assert_eq!(code(&bad), "io.ifc.write_id_duplicate");
}

/// The bytes of the default (IFC4) path for the ramp, curtain wall and grid model stay as written.
#[test]
fn ramp_output_bytes_are_pinned() {
    let bytes = write(&model(), &options()).unwrap().bytes;
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    assert_eq!(hash, 12_422_165_792_295_622_897, "{} bytes", bytes.len());
}

/// Every model of this file in IFC4X3 and IFC2X3: written, self-checked and dumped for the oracle.
#[test]
fn models_write_in_every_schema() {
    use arc_io_ifc::write::OutputSchema;
    let models: Vec<(&str, WriteModel)> = vec![("ramps", model())];
    for (name, model) in &models {
        for (schema, tag, available) in [
            (OutputSchema::Ifc4, "ifc4", true),
            (OutputSchema::Ifc4x3, "ifc4x3", true),
            (
                OutputSchema::Ifc2x3,
                "ifc2x3",
                arc_io_ifc::write::OutputSchema::Ifc2x3.available(),
            ),
        ] {
            if !available {
                continue;
            }
            let mut options = options();
            options.schema = schema;
            let output = write(model, &options).unwrap_or_else(|e| panic!("{name} {tag}: {e:?}"));
            support::dump(&format!("{name}-{tag}.ifc"), &output.bytes);
            assert!(output.findings.is_empty(), "{name} {tag}");
        }
    }
}
