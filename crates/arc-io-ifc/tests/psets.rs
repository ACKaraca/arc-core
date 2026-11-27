//! The generated property and quantity set templates. They exist only where
//! `cargo xtask ifc-psets` was run; with `ARC_IFC_PSD_DIR` set they are required, otherwise the
//! tests skip.
//!
//! With `ARC_IFC_PSET_DUMP=<file>` the templates are written as JSON for
//! `tools/ifc-oracle/pset_diff.py`.

// Test helpers report skips on the terminal and stop on malformed tables.
#![allow(clippy::print_stderr, clippy::unwrap_used)]

use arc_io_ifc::Schema;
use arc_io_ifc::psets::{PropertyKind, PropertyTemplate, PsetTemplates, SetKind};
use serde_json::{Map, Value, json};

fn templates() -> Option<[(&'static str, &'static PsetTemplates); 2]> {
    match (
        PsetTemplates::of(&Schema::Ifc2x3),
        PsetTemplates::of(&Schema::Ifc4),
    ) {
        (Some(a), Some(b)) => Some([("IFC2X3", a), ("IFC4", b)]),
        _ => {
            assert!(
                std::env::var_os("ARC_IFC_PSD_DIR").is_none(),
                "ARC_IFC_PSD_DIR is set but the templates are not generated: run cargo xtask ifc-psets"
            );
            eprintln!("skipped: pset templates not generated (cargo xtask ifc-psets)");
            None
        }
    }
}

fn kind(p: &PropertyTemplate) -> Value {
    let (name, types, extra) = match &p.kind {
        PropertyKind::Single { data_type } => ("P_SINGLEVALUE", vec![*data_type], json!(null)),
        PropertyKind::Enumerated { enumeration, items } => (
            "P_ENUMERATEDVALUE",
            vec![],
            json!({"enumeration": enumeration, "items": items}),
        ),
        PropertyKind::Bounded { data_type } => ("P_BOUNDEDVALUE", vec![*data_type], json!(null)),
        PropertyKind::List { data_type } => ("P_LISTVALUE", vec![*data_type], json!(null)),
        PropertyKind::Table { defining, defined } => {
            ("P_TABLEVALUE", vec![*defining, *defined], json!(null))
        }
        PropertyKind::Reference { entity } => ("P_REFERENCEVALUE", vec![*entity], json!(null)),
        PropertyKind::Complex { name, properties } => (
            "P_COMPLEX",
            vec![*name],
            Value::Object(
                properties
                    .iter()
                    .map(|p| (p.name.to_owned(), kind(p)))
                    .collect(),
            ),
        ),
        PropertyKind::Quantity(q) => {
            let name = match q {
                arc_io_ifc::QuantityKind::Length => "Q_LENGTH",
                arc_io_ifc::QuantityKind::Area => "Q_AREA",
                arc_io_ifc::QuantityKind::Volume => "Q_VOLUME",
                arc_io_ifc::QuantityKind::Count => "Q_COUNT",
                arc_io_ifc::QuantityKind::Weight => "Q_WEIGHT",
                arc_io_ifc::QuantityKind::Time => "Q_TIME",
            };
            (name, vec![], json!(null))
        }
    };
    json!({"kind": name, "types": types, "extra": extra})
}

#[test]
fn templates_are_sorted_resolvable_and_complete() {
    let Some(all) = templates() else { return };
    let mut dump = Map::new();
    for (key, t) in all {
        assert_eq!(t.schema, key);
        let upper = |n: &str| n.to_ascii_uppercase();
        assert!(
            t.sets
                .windows(2)
                .all(|w| upper(w[0].name) < upper(w[1].name))
        );
        let mut sets = Map::new();
        for set in t.sets {
            assert!(std::ptr::eq(t.get(&upper(set.name)).unwrap(), set));
            assert_eq!(
                set.kind == SetKind::Quantities,
                set.properties
                    .iter()
                    .all(|p| matches!(p.kind, PropertyKind::Quantity(_))),
                "{}",
                set.name
            );
            let properties: Map<String, Value> = set
                .properties
                .iter()
                .map(|p| (p.name.to_owned(), kind(p)))
                .collect();
            let applicable: Vec<String> = set
                .applicable
                .iter()
                .map(|a| match a.predefined_type {
                    Some(p) => format!("{}/{p}", a.entity),
                    None => a.entity.to_owned(),
                })
                .collect();
            sets.insert(
                set.name.to_owned(),
                json!({"template_type": set.template_type, "applicable": applicable, "properties": properties}),
            );
        }
        dump.insert(key.to_owned(), Value::Object(sets));
    }
    // The published releases: IFC2X3 TC1 has 317 property sets and no quantity sets, IFC4 ADD2
    // TC1 has 420 property sets and 93 quantity sets.
    let count = |t: &PsetTemplates, k| t.sets.iter().filter(|s| s.kind == k).count();
    assert_eq!(count(all[0].1, SetKind::Properties), 317);
    assert_eq!(count(all[0].1, SetKind::Quantities), 0);
    assert_eq!(count(all[1].1, SetKind::Properties), 420);
    assert_eq!(count(all[1].1, SetKind::Quantities), 93);
    let wall = all[1].1.get("Pset_WallCommon").unwrap();
    assert!(matches!(
        wall.property("ThermalTransmittance").unwrap().kind,
        PropertyKind::Single {
            data_type: "IfcThermalTransmittanceMeasure"
        }
    ));
    if let Some(path) = std::env::var_os("ARC_IFC_PSET_DUMP") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&Value::Object(dump)).unwrap(),
        )
        .unwrap();
    }
}
