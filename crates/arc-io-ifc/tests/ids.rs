//! The IDS reader and checker on fixtures written for this project (IFC4X3, whose tables are part
//! of every build): facets, restrictions, cardinalities, units, inheritance from types, and
//! malformed or hostile documents.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::ids::{self, Status};
use arc_io_ifc::{IfcFile, ReadOptions, read};

const MODEL: &str = "\
#1=IFCPROJECT('p',$,'P',$,$,$,$,$,#2);\n#2=IFCUNITASSIGNMENT((#3,#4));\n\
#3=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n#4=IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);\n\
#10=IFCWALL('w1',$,'W1',$,$,$,$,$,.SHEAR.);\n#11=IFCWALL('w2',$,'W2','Load bearing',$,$,$,$,.NOTDEFINED.);\n\
#12=IFCSLAB('s1',$,$,$,$,$,$,$,.FLOOR.);\n\
#20=IFCPROPERTYSINGLEVALUE('Height',$,IFCLENGTHMEASURE(2500.),$);\n\
#21=IFCPROPERTYSINGLEVALUE('Rating',$,IFCLABEL('EI60'),$);\n\
#22=IFCPROPERTYSET('ps1',$,'Pset_Test',$,(#20,#21));\n\
#23=IFCRELDEFINESBYPROPERTIES('r1',$,$,$,(#10),#22);\n\
#30=IFCMATERIAL('Concrete',$,'concrete');\n#31=IFCRELASSOCIATESMATERIAL('r2',$,$,$,(#10),#30);\n\
#40=IFCCLASSIFICATION($,$,$,'Uniclass',$,$,$);\n\
#41=IFCCLASSIFICATIONREFERENCE($,'EF_25_10',$,#40,$,$);\n\
#42=IFCCLASSIFICATIONREFERENCE($,'EF_25_10_25',$,#41,$,$);\n\
#43=IFCRELASSOCIATESCLASSIFICATION('r3',$,$,$,(#10),#42);\n\
#50=IFCBUILDINGSTOREY('st',$,'Level 1',$,$,$,$,$,$,$);\n\
#51=IFCRELCONTAINEDINSPATIALSTRUCTURE('r4',$,$,$,(#10,#12),#50);\n\
#60=IFCWALLTYPE('wt',$,'Type',$,$,(#62),$,$,$,.SHEAR.);\n\
#61=IFCRELDEFINESBYTYPE('r5',$,$,$,(#11),#60);\n\
#62=IFCPROPERTYSET('ps2',$,'Pset_Test',$,(#63));\n\
#63=IFCPROPERTYSINGLEVALUE('Rating',$,IFCLABEL('EI30'),$);\n";

fn model() -> IfcFile {
    let text = format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('a.ifc','',(),(),'','','');\n\
         FILE_SCHEMA(('IFC4X3_ADD2'));\nENDSEC;\nDATA;\n{MODEL}ENDSEC;\nEND-ISO-10303-21;\n"
    );
    read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file
}

fn document(specification: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ids xmlns=\"http://standards.buildingsmart.org/IDS\" xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n\
         <info><title>T</title></info><specifications>{specification}</specifications></ids>"
    )
}

fn simple(tag: &str, value: &str) -> String {
    format!("<{tag}><simpleValue>{value}</simpleValue></{tag}>")
}

fn entity(name: &str) -> String {
    format!("<entity>{}</entity>", simple("name", name))
}

fn spec(applicability: &str, requirements: &str, occurs: &str) -> String {
    format!(
        "<specification name=\"S\" ifcVersion=\"IFC4X3_ADD2\"><applicability {occurs}>{applicability}</applicability>\
         <requirements>{requirements}</requirements></specification>"
    )
}

fn run(specification: &str) -> ids::SpecificationResult {
    let doc = ids::read(
        document(specification).as_bytes(),
        &Limits::DEFAULT,
        &Cancel::new(),
    )
    .unwrap();
    let report = doc
        .check(&model(), &Limits::DEFAULT, &Cancel::new())
        .unwrap();
    report.specifications.into_iter().next().unwrap()
}

const REQUIRED: &str = "minOccurs=\"1\" maxOccurs=\"unbounded\"";

#[test]
fn entity_attribute_and_predefined_type_facets() {
    let walls = run(&spec(&entity("IFCWALL"), "", REQUIRED));
    assert_eq!((walls.status, walls.applicable), (Status::Pass, 2));
    // Subclasses are not inherited: IFCBUILTELEMENT has no instance of its own.
    assert_eq!(
        run(&spec(&entity("IFCBUILTELEMENT"), "", REQUIRED)).status,
        Status::Fail
    );
    let shear = format!(
        "<entity>{}{}</entity>",
        simple("name", "IFCWALL"),
        simple("predefinedType", "SHEAR")
    );
    // The second wall is NOTDEFINED itself and takes SHEAR from its type.
    assert_eq!(run(&spec(&shear, "", REQUIRED)).applicable, 2);
    let userdefined = format!(
        "<entity>{}{}</entity>",
        simple("name", "IFCWALL"),
        simple("predefinedType", "PARTITIONING")
    );
    assert_eq!(run(&spec(&userdefined, "", REQUIRED)).applicable, 0);
    // Name is required: both walls have one; Description only the second.
    let named = format!("<attribute>{}</attribute>", simple("name", "Name"));
    assert_eq!(
        run(&spec(&entity("IFCWALL"), &named, REQUIRED)).status,
        Status::Pass
    );
    let described = format!("<attribute>{}</attribute>", simple("name", "Description"));
    let result = run(&spec(&entity("IFCWALL"), &described, REQUIRED));
    assert_eq!((result.status, result.failure_count), (Status::Fail, 1));
    assert_eq!(result.failures[0].instance, 10);
    let optional = "<attribute cardinality=\"optional\"><name><simpleValue>Description</simpleValue></name>\
                    <value><simpleValue>Load bearing</simpleValue></value></attribute>";
    assert_eq!(
        run(&spec(&entity("IFCWALL"), optional, REQUIRED)).status,
        Status::Pass
    );
    let prohibited = "<attribute cardinality=\"prohibited\"><name><simpleValue>Description</simpleValue></name></attribute>";
    assert_eq!(
        run(&spec(&entity("IFCWALL"), prohibited, REQUIRED)).failure_count,
        1
    );
}

#[test]
fn specification_cardinality() {
    assert_eq!(
        run(&spec(&entity("IFCDOOR"), "", REQUIRED)).status,
        Status::Fail
    );
    assert_eq!(
        run(&spec(
            &entity("IFCDOOR"),
            "",
            "minOccurs=\"0\" maxOccurs=\"unbounded\""
        ))
        .status,
        Status::Pass
    );
    assert_eq!(
        run(&spec(
            &entity("IFCWALL"),
            "",
            "minOccurs=\"0\" maxOccurs=\"0\""
        ))
        .status,
        Status::Fail
    );
    let doors = run(&spec(
        &entity("IFCDOOR"),
        "",
        "minOccurs=\"0\" maxOccurs=\"0\"",
    ));
    assert_eq!(doors.status, Status::Pass);
}

#[test]
fn property_facets_convert_units_and_inherit_from_types() {
    let property = |set: &str, name: &str, value: Option<&str>, data_type: &str| {
        format!(
            "<property dataType=\"{data_type}\">{}{}{}</property>",
            simple("propertySet", set),
            simple("baseName", name),
            value.map_or(String::new(), |v| simple("value", v)),
        )
    };
    // 2500 mm is 2.5 m.
    let height = property("Pset_Test", "Height", Some("2.5"), "IFCLENGTHMEASURE");
    assert_eq!(run(&spec(&entity("IFCWALL"), &height, "")).applicable, 2);
    let result = run(&spec(&entity("IFCWALL"), &height, REQUIRED));
    // The second wall has no Height at all.
    assert_eq!((result.status, result.failure_count), (Status::Fail, 1));
    assert_eq!(result.failures[0].instance, 11);
    let wrong = property("Pset_Test", "Height", Some("2500"), "IFCLENGTHMEASURE");
    assert_eq!(
        run(&spec(&entity("IFCWALL"), &wrong, REQUIRED)).failure_count,
        2
    );
    // The second wall takes Rating from its type, the first from itself.
    let rating = property("Pset_Test", "Rating", None, "IFCLABEL");
    assert_eq!(
        run(&spec(&entity("IFCWALL"), &rating, REQUIRED)).status,
        Status::Pass
    );
    let ei30 = property("Pset_Test", "Rating", Some("EI30"), "IFCLABEL");
    let result = run(&spec(&entity("IFCWALL"), &ei30, REQUIRED));
    assert_eq!(
        result
            .failures
            .iter()
            .map(|f| f.instance)
            .collect::<Vec<_>>(),
        [10]
    );
    // A pattern restriction.
    let pattern = "<property dataType=\"IFCLABEL\"><propertySet><simpleValue>Pset_Test</simpleValue></propertySet>\
        <baseName><simpleValue>Rating</simpleValue></baseName><value><xs:restriction base=\"xs:string\">\
        <xs:pattern value=\"EI[0-9]{2}\"/></xs:restriction></value></property>";
    assert_eq!(
        run(&spec(&entity("IFCWALL"), pattern, REQUIRED)).status,
        Status::Pass
    );
    // A bound with the tolerance does not apply to ranges.
    let range = "<property dataType=\"IFCLENGTHMEASURE\"><propertySet><simpleValue>Pset_Test</simpleValue></propertySet>\
        <baseName><simpleValue>Height</simpleValue></baseName><value><xs:restriction base=\"xs:double\">\
        <xs:minExclusive value=\"2.5\"/></xs:restriction></value></property>";
    let first_wall = spec(
        &format!(
            "{}{}",
            entity("IFCWALL"),
            format_args!("<attribute>{}</attribute>", simple("name", "Name"))
        ),
        range,
        REQUIRED,
    );
    assert_eq!(run(&first_wall).status, Status::Fail);
}

#[test]
fn material_classification_and_part_of_facets() {
    let material = "<material><value><simpleValue>concrete</simpleValue></value></material>";
    let result = run(&spec(&entity("IFCWALL"), material, REQUIRED));
    assert_eq!(
        result
            .failures
            .iter()
            .map(|f| f.instance)
            .collect::<Vec<_>>(),
        [11]
    );
    // A material facet in the applicability selects the elements that have one.
    assert_eq!(
        run(&spec(
            &format!("{}{}", entity("IFCWALL"), "<material/>"),
            "",
            REQUIRED
        ))
        .applicable,
        1
    );
    // A classification reaches up its references: the system is the root's name.
    let classification = "<classification><value><simpleValue>EF_25_10</simpleValue></value>\
        <system><simpleValue>Uniclass</simpleValue></system></classification>";
    assert_eq!(
        run(&spec(&entity("IFCWALL"), classification, REQUIRED)).failure_count,
        1
    );
    let part_of = "<partOf relation=\"IFCRELCONTAINEDINSPATIALSTRUCTURE\"><entity><name><simpleValue>IFCBUILDINGSTOREY</simpleValue></name></entity></partOf>";
    let result = run(&spec(&entity("IFCSLAB"), part_of, REQUIRED));
    assert_eq!(result.status, Status::Pass);
    assert_eq!(
        run(&spec(&entity("IFCWALL"), part_of, REQUIRED)).failure_count,
        1
    );
}

#[test]
fn the_reader_rejects_documents_outside_the_schema() {
    let limits = Limits::DEFAULT;
    let cancel = Cancel::new();
    let bad = [
        // Wrong namespace.
        "<ids xmlns=\"urn:x\"><info><title>T</title></info><specifications/></ids>".to_owned(),
        // An unknown element in a facet.
        document(&spec(
            "<entity><name><simpleValue>IFCWALL</simpleValue></name><bogus/></entity>",
            "",
            REQUIRED,
        )),
        // A prohibited specification with requirements.
        document(&spec(
            &entity("IFCWALL"),
            &format!("<attribute>{}</attribute>", simple("name", "Name")),
            "minOccurs=\"0\" maxOccurs=\"0\"",
        )),
        // A pattern outside the pattern language.
        document(&spec(
            &entity("IFCWALL"),
            "<attribute><name><xs:restriction base=\"xs:string\"><xs:pattern value=\"(?:a)\"/></xs:restriction></name></attribute>",
            REQUIRED,
        )),
        // ifcVersion missing.
        document(
            "<specification name=\"S\"><applicability minOccurs=\"1\" maxOccurs=\"unbounded\"><entity><name><simpleValue>IFCWALL</simpleValue></name></entity></applicability></specification>",
        ),
        "not xml".to_owned(),
    ];
    for text in &bad {
        let error = ids::read(text.as_bytes(), &limits, &cancel).unwrap_err();
        assert!(error.code().starts_with("io."), "{text}: {}", error.code());
    }
    let digits = document(&spec(
        &entity("IFCWALL"),
        "<attribute><name><xs:restriction base=\"xs:string\"><xs:totalDigits value=\"3\"/></xs:restriction></name></attribute>",
        REQUIRED,
    ));
    assert_eq!(
        ids::read(digits.as_bytes(), &limits, &cancel)
            .unwrap_err()
            .code(),
        "io.ids.unsupported"
    );
    // A document type declaration (entity expansion) is refused by the shared reader.
    let doctype = format!(
        "<!DOCTYPE ids [<!ENTITY a \"b\">]>{}",
        document(&spec(&entity("IFCWALL"), "", REQUIRED))
    );
    assert!(ids::read(doctype.as_bytes(), &limits, &cancel).is_err());
}

#[test]
fn hostile_documents_are_bounded() {
    let cancel = Cancel::new();
    // Nesting beyond the limit.
    let deep = format!("{}{}", "<a>".repeat(10_000), "</a>".repeat(10_000));
    assert!(ids::read(deep.as_bytes(), &Limits::CONSTRAINED, &cancel).is_err());
    // More elements than record_count.
    let many = document(&spec(&entity("IFCWALL"), "", REQUIRED).repeat(5_000));
    let tiny = Limits {
        record_count: 1_000,
        ..Limits::DEFAULT
    };
    assert_eq!(
        ids::read(many.as_bytes(), &tiny, &cancel)
            .unwrap_err()
            .code(),
        "io.limit_exceeded"
    );
    let stop = Cancel::new();
    stop.cancel();
    let text = document(&spec(&entity("IFCWALL"), "", REQUIRED));
    assert_eq!(
        ids::read(text.as_bytes(), &Limits::DEFAULT, &stop)
            .unwrap_err()
            .code(),
        "base.cancelled"
    );
    // A cancelled check.
    let doc = ids::read(text.as_bytes(), &Limits::DEFAULT, &cancel).unwrap();
    assert_eq!(
        doc.check(&model(), &Limits::DEFAULT, &stop)
            .unwrap_err()
            .code(),
        "base.cancelled"
    );
}
