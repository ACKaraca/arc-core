//! Placement and membership rules of the typed views, with small fixtures: a second parent is a
//! duplicate placement and not a cycle, elements outside the spatial structure are not mapped,
//! missing or wrongly typed members are reported, one bad member drops only itself, and the IFC4
//! definition-set form is read as its members.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::loss::Argument;
use arc_io_ifc::{ReadOptions, TypedViews, read, reasons};

fn file(data: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn views(data: &str) -> TypedViews {
    let output = read(file(data).as_bytes(), &mut ReadOptions::default()).unwrap();
    output
        .file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap()
}

fn count_of(views: &TypedViews, reason: &str) -> i64 {
    views
        .report
        .items
        .iter()
        .filter(|item| item.reason == reason)
        .map(|item| match item.arguments.get("count") {
            Some(Argument::Int(count)) => *count,
            _ => 0,
        })
        .sum()
}

/// A project, a site and a building: the spatial part shared by the placement fixtures.
const SPATIAL: &str = "#1=IFCPROJECT('p',$,'P',$,$,$,$,$,$);\n\
#2=IFCSITE('s',$,'S',$,$,$,$,$,$,$,$,$,$,$);\n#3=IFCBUILDING('b',$,'B',$,$,$,$,$,$,$,$,$);\n";

#[test]
fn second_parent_is_a_duplicate_placement_not_a_cycle() {
    let views = views(&format!(
        "{SPATIAL}#10=IFCRELAGGREGATES('a1',$,$,$,#1,(#2,#3));\n#11=IFCRELAGGREGATES('a2',$,$,$,#2,(#3));"
    ));
    assert_eq!(views.spatial[0].children.len(), 1);
    assert_eq!(
        views.spatial[0].children[0].children[0]
            .entity
            .name
            .as_deref(),
        Some("B")
    );
    assert_eq!(count_of(&views, reasons::DUPLICATE_PLACEMENT.code), 1);
    assert_eq!(count_of(&views, reasons::CYCLE_CUT.code), 0);
}

#[test]
fn aggregation_of_a_non_spatial_element_is_not_mapped() {
    let views = views(&format!(
        "{SPATIAL}#4=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#10=IFCRELAGGREGATES('a',$,$,$,#1,(#2,#4));"
    ));
    assert_eq!(views.spatial[0].children.len(), 1);
    assert_eq!(count_of(&views, reasons::NOT_MAPPED.code), 1);
    assert_eq!(views.report.summary.preserved, 1);
}

#[test]
fn facility_types_join_the_spatial_structure() {
    let views = views(
        "#1=IFCPROJECT('p',$,'P',$,$,$,$,$,$);\n#2=IFCFACILITY('f',$,'Bridge 1',$,$,$,$,$,$,$,$);\n\
#10=IFCRELAGGREGATES('a',$,$,$,#1,(#2));",
    );
    assert_eq!(views.spatial[0].children.len(), 1);
    assert_eq!(views.spatial[0].children[0].entity.type_name, "IFCFACILITY");
    assert_eq!(count_of(&views, reasons::NOT_MAPPED.code), 0);
}

#[test]
fn missing_property_member_is_reported_and_the_others_are_kept() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#16=IFCPROPERTYSET('ps',$,'Pset_X',$,(#20,#99));\n\
#20=IFCPROPERTYSINGLEVALUE('A',$,IFCLABEL('x'),$);\n#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8),#16);"
    ));
    assert_eq!(views.property_sets.len(), 1);
    assert_eq!(views.property_sets[0].properties.len(), 1);
    assert_eq!(count_of(&views, reasons::TARGET_MISSING.code), 1);
}

#[test]
fn missing_definition_is_reported() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8),#99);"
    ));
    assert!(views.property_sets.is_empty());
    assert_eq!(count_of(&views, reasons::TARGET_MISSING.code), 1);
}

#[test]
fn one_bad_member_drops_only_itself() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#16=IFCPROPERTYSET('ps',$,'Pset_X',$,(#20,'bad',#21));\n\
#20=IFCPROPERTYSINGLEVALUE('A',$,IFCLABEL('x'),$);\n#21=IFCPROPERTYSINGLEVALUE('B',$,IFCLABEL('y'),$);\n\
#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8),#16);"
    ));
    assert_eq!(views.property_sets[0].properties.len(), 2);
    assert_eq!(count_of(&views, reasons::VALUE_UNEXPECTED.code), 1);
}

#[test]
fn ifc4_definition_set_is_read_as_its_members() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#16=IFCPROPERTYSET('ps',$,'Pset_X',$,(#20));\n\
#18=IFCPROPERTYSET('ps2',$,'Pset_Y',$,(#21));\n#20=IFCPROPERTYSINGLEVALUE('A',$,IFCLABEL('x'),$);\n\
#21=IFCPROPERTYSINGLEVALUE('B',$,IFCLABEL('y'),$);\n\
#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8),IFCPROPERTYSETDEFINITIONSET((#16,#18)));"
    ));
    let names: Vec<_> = views
        .property_sets
        .iter()
        .map(|set| set.definition.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names, ["Pset_X", "Pset_Y"]);
    assert!(views.property_sets.iter().all(|set| set.related.len() == 1));
}

#[test]
fn complex_property_is_not_mapped() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#16=IFCPROPERTYSET('ps',$,'Pset_X',$,(#20));\n\
#20=(IFCPROPERTYSINGLEVALUE('A',$,IFCLABEL('x'),$)IFCX(1));\n\
#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8),#16);"
    ));
    assert!(views.property_sets[0].properties.is_empty());
    assert_eq!(count_of(&views, reasons::NOT_MAPPED.code), 1);
}

#[test]
fn repeated_property_in_has_properties_is_read_once() {
    let views = views(&format!(
        "{SPATIAL}#8=IFCWALL('w',$,'W',$,$,$,$,$,$);\n#16=IFCPROPERTYSET('ps',$,'Pset_X',$,(#20,#20,#20));\n\
#20=IFCPROPERTYSINGLEVALUE('A',$,IFCLABEL('x'),$);\n#17=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#8,#8),#16);"
    ));
    assert_eq!(views.property_sets[0].properties.len(), 1);
    assert_eq!(views.property_sets[0].related.len(), 1);
    assert_eq!(count_of(&views, reasons::DUPLICATE_PLACEMENT.code), 3);
}
