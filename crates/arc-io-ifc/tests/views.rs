//! The typed views of the minimal project, in each of the three schema identifiers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

#[path = "support/mod.rs"]
mod support;

use arc_base::{Cancel, Limits};
use arc_io_ifc::{Number, PropertyValue, QuantityKind, ReadOptions, Schema, Value, read, reasons};

/// The file, the typed views and the loss report of the read.
fn views(
    schema: &str,
) -> (
    arc_io_ifc::IfcFile,
    arc_io_ifc::TypedViews,
    arc_io_ifc::LossReport,
) {
    let output = read(
        support::project(schema).as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    let views = output
        .file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    (output.file, views, output.report)
}

fn check_project(schema: &str, expected: Schema) {
    let (file, views, read_report) = views(schema);
    assert_eq!(file.schema, expected);

    // The spatial tree: project, site, building, two storeys, the space under the ground floor.
    assert_eq!(views.spatial.len(), 1);
    let project = &views.spatial[0];
    assert_eq!(project.entity.type_name, "IFCPROJECT");
    assert_eq!(
        project.entity.global_id.as_deref(),
        Some("0YvctVUKr0kugbFTf53O9L")
    );
    assert_eq!(project.entity.name.as_deref(), Some("Demo Project"));
    assert_eq!(
        project.entity.description.as_deref(),
        Some("Project description")
    );
    let site = &project.children[0];
    assert_eq!(site.entity.name.as_deref(), Some("Site A"));
    let building = &site.children[0];
    assert_eq!(building.entity.type_name, "IFCBUILDING");
    let names: Vec<_> = building
        .children
        .iter()
        .map(|s| s.entity.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names, ["Ground", "First"]);
    let ground = &building.children[0];
    assert_eq!(ground.children[0].entity.name.as_deref(), Some("Room 1"));
    assert_eq!(ground.children[0].entity.type_name, "IFCSPACE");

    // The walls are contained in the first storey, in file order.
    let first = &building.children[1];
    let walls: Vec<_> = first
        .contained
        .iter()
        .map(|e| (e.global_id.clone(), e.name.clone(), e.description.clone()))
        .collect();
    assert_eq!(
        walls,
        [
            (
                Some("7Lj9hJ7xJ5rB6tPcY4E5iU".to_owned()),
                Some("Wall A".to_owned()),
                Some("North elevation".to_owned())
            ),
            (
                Some("8Mk0iK8yK6sC7uQdZ5F6jV".to_owned()),
                Some("Wall B".to_owned()),
                None
            ),
        ]
    );

    // Both walls have the one wall type.
    assert_eq!(views.types.len(), 2);
    for assignment in &views.types {
        assert_eq!(assignment.element_type.type_name, "IFCWALLTYPE");
        assert_eq!(assignment.element_type.name.as_deref(), Some("Brick wall"));
    }
    assert_eq!(views.types[1].element.name.as_deref(), Some("Wall B"));

    // The property set: one property of each kind, in order.
    assert_eq!(views.property_sets.len(), 1);
    let pset = &views.property_sets[0];
    assert_eq!(pset.definition.name.as_deref(), Some("Pset_WallCommon"));
    assert_eq!(pset.related.len(), 2);
    let props = &pset.properties;
    assert_eq!(props.len(), 5);
    assert_eq!(props[0].name.as_deref(), Some("IsExternal"));
    match &props[0].value {
        PropertyValue::Single { nominal, unit } => {
            let nominal = nominal.as_ref().unwrap();
            assert_eq!(nominal.type_name.as_deref(), Some("IFCBOOLEAN"));
            assert_eq!(nominal.value, Value::Enumeration("T".to_owned()));
            assert_eq!(*unit, None);
        }
        other => panic!("single value expected, got {other:?}"),
    }
    match &props[1].value {
        PropertyValue::Single { nominal, .. } => {
            let nominal = nominal.as_ref().unwrap();
            assert_eq!(nominal.type_name.as_deref(), Some("IFCLABEL"));
            assert_eq!(nominal.value, Value::Text("2HR".to_owned()));
        }
        other => panic!("single value expected, got {other:?}"),
    }
    match &props[2].value {
        PropertyValue::Enumerated { values, reference } => {
            assert_eq!(values.len(), 2);
            assert_eq!(values[1].value, Value::Text("Tile".to_owned()));
            assert_eq!(*reference, None);
        }
        other => panic!("enumerated value expected, got {other:?}"),
    }
    match &props[3].value {
        PropertyValue::List { values, unit } => {
            assert_eq!(values[0].value, Value::Real(1.5));
            assert_eq!(values[1].type_name.as_deref(), Some("IFCREAL"));
            assert_eq!(*unit, None);
        }
        other => panic!("list value expected, got {other:?}"),
    }
    match &props[4].value {
        PropertyValue::Bounded {
            upper,
            lower,
            unit,
            set_point,
        } => {
            assert_eq!(upper.as_ref().unwrap().value, Value::Real(300.0));
            assert_eq!(lower.as_ref().unwrap().value, Value::Real(100.0));
            // IFC2X3 has no SetPointValue; the other schemas carry it.
            if expected == Schema::Ifc2x3 {
                assert!(set_point.is_none());
            } else {
                assert_eq!(set_point.as_ref().unwrap().value, Value::Real(200.0));
            }
            assert_eq!(*unit, Some(30));
        }
        other => panic!("bounded value expected, got {other:?}"),
    }

    // The quantity set: one quantity of each kind, with unit, value and formula.
    assert_eq!(views.quantity_sets.len(), 1);
    let qset = &views.quantity_sets[0];
    assert_eq!(
        qset.definition.name.as_deref(),
        Some("Qto_WallBaseQuantities")
    );
    assert_eq!(qset.related.len(), 2);
    let kinds: Vec<_> = qset.quantities.iter().map(|q| q.kind).collect();
    assert_eq!(
        kinds,
        [
            QuantityKind::Length,
            QuantityKind::Area,
            QuantityKind::Volume,
            QuantityKind::Count,
            QuantityKind::Weight,
            QuantityKind::Time,
        ]
    );
    assert_eq!(qset.quantities[0].unit, Some(30));
    assert_eq!(qset.quantities[0].value, Some(Number::Real(2500.0)));
    assert_eq!(qset.quantities[1].value, Some(Number::Real(12.5)));
    assert_eq!(qset.quantities[3].value, Some(Number::Integer(4)));
    assert_eq!(qset.quantities[3].name.as_deref(), Some("Count"));
    assert_eq!(qset.quantities[5].value, Some(Number::Real(3600.0)));

    // Nothing was dropped by the views; the unit is an entity they do not read, kept as preserved.
    assert_eq!(views.report.summary.dropped, 0);
    assert!(
        read_report
            .items
            .iter()
            .any(|i| i.reason == reasons::ENTITY_PRESERVED.code
                && i.record_kind.as_deref() == Some("IFCSIUNIT"))
    );
}

#[test]
fn minimal_project_in_ifc2x3() {
    check_project("IFC2X3", Schema::Ifc2x3);
}

#[test]
fn minimal_project_in_ifc4() {
    check_project("IFC4", Schema::Ifc4);
}

#[test]
fn minimal_project_in_ifc4x3() {
    check_project("IFC4X3", Schema::Ifc4x3);
}

#[test]
fn schema_suffixes_and_unknown_schemas() {
    let (file, _, _) = views("IFC4X3_ADD2");
    assert_eq!(file.schema, Schema::Ifc4x3);
    let (file, _, report) = views("IFC5");
    assert_eq!(file.schema, Schema::Other("IFC5".to_owned()));
    assert!(
        report
            .items
            .iter()
            .any(|i| i.reason == reasons::SCHEMA_UNRECOGNIZED.code)
    );
}

#[test]
fn instances_are_indexed_by_name_and_by_type() {
    let (file, _, _) = views("IFC4");
    assert_eq!(file.instance(8).unwrap().records[0].type_name, "IFCWALL");
    assert!(file.instance(999).is_none());
    let walls: Vec<u64> = file.instances_of("ifcwall").map(|i| i.name).collect();
    assert_eq!(walls, [8, 9]);
    assert_eq!(file.instances_of("IFCNOTHING").count(), 0);
}
