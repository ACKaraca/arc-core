//! The generated schema tables (ADR-0018). They exist only where `cargo xtask ifc-schema` was run;
//! with `ARC_IFC_SCHEMA_DIR` set they are required, otherwise the tests skip.
//!
//! With `ARC_IFC_SCHEMA_DUMP=<file>` the tables are written as JSON for
//! `tools/ifc-oracle/schema_diff.py`.

// Test helpers report skips on the terminal and stop on malformed tables.
#![allow(clippy::print_stderr, clippy::unwrap_used)]

use arc_io_ifc::Schema;
use arc_io_ifc::schema::{AggregateKind, Bound, Inverse, Named, SchemaTables, TypeRef};
use serde_json::{Value, json};

const SCHEMAS: [(Schema, &str); 3] = [
    (Schema::Ifc2x3, "IFC2X3"),
    (Schema::Ifc4, "IFC4"),
    (Schema::Ifc4x3, "IFC4X3"),
];

fn tables() -> Option<Vec<(&'static str, &'static SchemaTables)>> {
    let found: Option<Vec<_>> = SCHEMAS
        .iter()
        .map(|(s, key)| SchemaTables::of(s).map(|t| (*key, t)))
        .collect();
    if found.is_none() {
        assert!(
            std::env::var_os("ARC_IFC_SCHEMA_DIR").is_none(),
            "ARC_IFC_SCHEMA_DIR is set but the tables are not generated: run cargo xtask ifc-schema"
        );
        eprintln!("skipped: schema tables not generated (cargo xtask ifc-schema)");
    }
    found
}

#[test]
fn tables_are_sorted_and_consistent() {
    let Some(all) = tables() else { return };
    for (key, t) in all {
        let upper = |n: &str| n.to_ascii_uppercase();
        assert!(
            t.entities
                .windows(2)
                .all(|w| upper(w[0].name) < upper(w[1].name)),
            "{key}"
        );
        assert!(
            t.enumerations
                .windows(2)
                .all(|w| upper(w[0].name) < upper(w[1].name))
        );
        for (i, e) in t.entities.iter().enumerate() {
            let i = u16::try_from(i).unwrap();
            assert_eq!(t.entity_index(&upper(e.name)), Some(i));
            assert_eq!(t.find(&e.name.to_ascii_lowercase()), Some(Named::Entity(i)));
            for &sub in e.one_of {
                assert_eq!(
                    t.entities[usize::from(sub)].supertype,
                    Some(i),
                    "{key} {}",
                    e.name
                );
            }
        }
    }
}

#[test]
fn known_entities_resolve() {
    let Some(all) = tables() else { return };
    let ifc4 = all[1].1;
    assert_eq!(ifc4.name, "IFC4");
    let wall = ifc4.entity_index("IFCWALL").unwrap();
    let root = ifc4.entity_index("IfcRoot").unwrap();
    assert!(ifc4.is_subtype_of(wall, root));
    let positions = ifc4.positions(wall);
    let names: Vec<_> = positions.iter().map(|p| p.attribute.name).collect();
    assert_eq!(names[0], "GlobalId");
    assert_eq!(names.last(), Some(&"PredefinedType"));
    assert_eq!(names.len(), 9);
    // IfcSIUnit re-declares the dimensions of IfcNamedUnit as derived.
    let si = ifc4.entity_index("IfcSIUnit").unwrap();
    let derived: Vec<_> = ifc4
        .positions(si)
        .iter()
        .map(|p| (p.attribute.name, p.derived))
        .collect();
    assert_eq!(derived[0], ("Dimensions", true));
    assert!(
        ifc4.enumeration("IfcWallTypeEnum")
            .unwrap()
            .contains("partitioning")
    );
    assert!(ifc4.function("IfcCorrectDimensions").is_some());
    assert!(ifc4.rule("IfcSingleProjectInstance").is_some());
    assert!(ifc4.entity("IfcDoorStyle").is_some());
    assert!(all[2].1.entity("IfcDoorStyle").is_none());
    assert!(all[2].1.entity("IfcAlignment").is_some());
}

fn bound(b: Bound) -> Value {
    match b {
        Bound::Value(v) => json!(v),
        Bound::Unbounded => Value::Null,
        Bound::Expression(e) => json!(e),
    }
}

fn type_name(t: &SchemaTables, ty: TypeRef) -> Value {
    let name = |n| match n {
        Named::Entity(i) => t.entities[usize::from(i)].name,
        Named::Defined(i) => t.defined_types[usize::from(i)].name,
        Named::Enumeration(i) => t.enumerations[usize::from(i)].name,
        Named::Select(i) => t.selects[usize::from(i)].name,
    };
    match ty {
        TypeRef::Named(n) => json!(name(n)),
        TypeRef::Aggregate(a) => json!({
            "kind": format!("{:?}", a.kind).to_ascii_uppercase(),
            "lower": bound(a.lower), "upper": bound(a.upper),
            "unique": a.unique, "optional": a.optional,
            "element": type_name(t, a.element),
        }),
        other => json!(format!("{other:?}").to_ascii_uppercase()),
    }
}

fn inverse(t: &SchemaTables, i: &Inverse) -> Value {
    let (kind, lower, upper) = match i.aggregate {
        Some(a) => (
            if a.kind == AggregateKind::Bag {
                "BAG"
            } else {
                "SET"
            },
            bound(a.lower),
            bound(a.upper),
        ),
        None => ("", json!(1), json!(1)),
    };
    json!({"name": i.name, "entity": t.entities[usize::from(i.entity)].name,
           "attribute": i.attribute, "kind": kind, "lower": lower, "upper": upper})
}

/// The tables as JSON in the shape `tools/ifc-oracle/schema_diff.py` reads, with the record
/// positions, inverse attributes, enumerations and selects added.
fn dump(t: &SchemaTables) -> Value {
    let mut entities = serde_json::Map::new();
    for (i, e) in t.entities.iter().enumerate() {
        let positions = t.positions(u16::try_from(i).unwrap());
        entities.insert(e.name.to_owned(), json!({
            "supertype": e.supertype.map(|s| t.entities[usize::from(s)].name),
            "abstract": e.is_abstract,
            "attributes": e.attributes.iter().map(|a| json!({
                "name": a.name, "optional": a.optional, "type": type_name(t, a.ty)})).collect::<Vec<_>>(),
            "positions": positions.iter().map(|p| json!([p.attribute.name, p.derived])).collect::<Vec<_>>(),
            "inverses": e.inverses.iter().map(|i| inverse(t, i)).collect::<Vec<_>>(),
        }));
    }
    let enumerations: serde_json::Map<_, _> = t
        .enumerations
        .iter()
        .map(|e| (e.name.to_owned(), json!(e.items)))
        .collect();
    let selects: serde_json::Map<_, _> = t
        .selects
        .iter()
        .map(|s| {
            let members: Vec<_> = s
                .members
                .iter()
                .map(|&m| type_name(t, TypeRef::Named(m)))
                .collect();
            (s.name.to_owned(), json!(members))
        })
        .collect();
    json!({"name": t.name, "entities": entities, "enumerations": enumerations, "selects": selects})
}

#[test]
fn dump_for_the_oracle() {
    let Some(path) = std::env::var_os("ARC_IFC_SCHEMA_DUMP") else {
        return;
    };
    let Some(all) = tables() else { return };
    let out: serde_json::Map<_, _> = all
        .iter()
        .map(|(k, t)| ((*k).to_owned(), dump(t)))
        .collect();
    std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
}
