#![allow(clippy::unwrap_used, reason = "test data")]

use super::*;
use crate::{ReadOptions, read};
use arc_base::{Cancel, Limits};

static TEMPLATES: PsetTemplates = PsetTemplates {
    schema: "TEST",
    sets: &[
        SetTemplate {
            name: "Pset_TestCommon",
            kind: SetKind::Properties,
            template_type: "PSET_TYPEDRIVENOVERRIDE",
            applicable: &[Applicable {
                entity: "IfcWall",
                predefined_type: None,
            }],
            properties: &[
                PropertyTemplate {
                    name: "Reference",
                    kind: PropertyKind::Single {
                        data_type: "IfcIdentifier",
                    },
                },
                PropertyTemplate {
                    name: "Status",
                    kind: PropertyKind::Enumerated {
                        enumeration: "PEnum_S",
                        items: &["NEW", "EXISTING"],
                    },
                },
                PropertyTemplate {
                    name: "Width",
                    kind: PropertyKind::Bounded {
                        data_type: "IfcPositiveLengthMeasure",
                    },
                },
                PropertyTemplate {
                    name: "Tags",
                    kind: PropertyKind::List {
                        data_type: "IfcLabel",
                    },
                },
                PropertyTemplate {
                    name: "Open",
                    kind: PropertyKind::Single { data_type: "" },
                },
                PropertyTemplate {
                    name: "Owner",
                    kind: PropertyKind::Reference {
                        entity: "IfcPerson",
                    },
                },
                PropertyTemplate {
                    name: "Cp",
                    kind: PropertyKind::Complex {
                        name: "CP_T",
                        properties: &[PropertyTemplate {
                            name: "Inner",
                            kind: PropertyKind::Single {
                                data_type: "IfcInteger",
                            },
                        }],
                    },
                },
            ],
        },
        SetTemplate {
            name: "Qto_TestBaseQuantities",
            kind: SetKind::Quantities,
            template_type: "",
            applicable: &[],
            properties: &[
                PropertyTemplate {
                    name: "Length",
                    kind: PropertyKind::Quantity(QuantityKind::Length),
                },
                PropertyTemplate {
                    name: "Count",
                    kind: PropertyKind::Quantity(QuantityKind::Count),
                },
            ],
        },
    ],
};

fn file(body: &str) -> crate::IfcFile {
    file_in("IFC4", body)
}

fn file_in(schema: &str, body: &str) -> crate::IfcFile {
    let text = format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('','',(''),(''),'','','');\n\
         FILE_SCHEMA(('{schema}'));\nENDSEC;\nDATA;\n{body}ENDSEC;\nEND-ISO-10303-21;\n"
    );
    read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file
}

fn run(body: &str) -> PsetCheck {
    check::check(
        &file(body),
        &TEMPLATES,
        &Limits::CONSTRAINED,
        &Cancel::new(),
    )
    .unwrap()
}

const GOOD: &str = "#1=IFCPROPERTYSINGLEVALUE('Reference',$,IFCIDENTIFIER('A-1'),$);
#2=IFCPROPERTYENUMERATEDVALUE('Status',$,(IFCLABEL('NEW')),$);
#3=IFCPROPERTYBOUNDEDVALUE('Width',$,IFCPOSITIVELENGTHMEASURE(2.),IFCPOSITIVELENGTHMEASURE(1.),$,$);
#4=IFCPROPERTYLISTVALUE('Tags',$,(IFCLABEL('a'),IFCLABEL('b')),$);
#5=IFCPROPERTYSINGLEVALUE('Open',$,IFCREAL(1.),$);
#6=IFCPERSON($,$,$,$,$,$,$,$);
#7=IFCPROPERTYREFERENCEVALUE('Owner',$,$,#6);
#8=IFCPROPERTYSINGLEVALUE('Inner',$,IFCINTEGER(3),$);
#9=IFCCOMPLEXPROPERTY('Cp',$,'CP_T',(#8));
#10=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L',$,'Pset_TestCommon',$,(#1,#2,#3,#4,#5,#7,#9));
#11=IFCQUANTITYLENGTH('Length',$,$,2.,$);
#12=IFCQUANTITYCOUNT('Count',$,$,4.,$);
#13=IFCELEMENTQUANTITY('1YvctVUKr0kugbFTf53O9L',$,'Qto_TestBaseQuantities',$,$,(#11,#12));
#14=IFCPROPERTYSET('2YvctVUKr0kugbFTf53O9L',$,'Custom',$,(#1));
";

#[test]
fn sets_matching_their_templates_have_no_findings() {
    let result = run(GOOD);
    assert_eq!(result.templates, Some("TEST"));
    assert_eq!(result.sets_checked, 2);
    assert_eq!(result.findings, []);
}

#[test]
fn every_difference_is_reported_with_a_stable_code() {
    let body = "#1=IFCPROPERTYSINGLEVALUE('Reference',$,IFCLABEL('A-1'),$);
#2=IFCPROPERTYENUMERATEDVALUE('Status',$,(IFCLABEL('OLD')),$);
#3=IFCPROPERTYSINGLEVALUE('Width',$,IFCLENGTHMEASURE(2.),$);
#4=IFCPROPERTYLISTVALUE('Tags',$,(IFCLABEL('a'),'b'),$);
#5=IFCPROPERTYSINGLEVALUE('Colour',$,IFCLABEL('red'),$);
#6=IFCORGANIZATION($,'O',$,$,$);
#7=IFCPROPERTYREFERENCEVALUE('Owner',$,$,#6);
#8=IFCPROPERTYSINGLEVALUE('Inner',$,IFCREAL(3.),$);
#9=IFCCOMPLEXPROPERTY('Cp',$,'CP_T',(#8));
#10=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L',$,'Pset_TestCommon',$,(#1,#2,#3,#4,#5,#7,#9));
#11=IFCQUANTITYAREA('Length',$,$,2.,$);
#12=IFCPROPERTYSINGLEVALUE('Count',$,IFCCOUNTMEASURE(4),$);
#13=IFCELEMENTQUANTITY('1YvctVUKr0kugbFTf53O9L',$,'Qto_TestBaseQuantities',$,$,(#11,#12));
#14=IFCPROPERTYSET('2YvctVUKr0kugbFTf53O9L',$,'Pset_Nope',$,(#1));
#15=IFCPROPERTYSET('3YvctVUKr0kugbFTf53O9L',$,'Qto_TestBaseQuantities',$,(#1));
";
    let result = run(body);
    let got: Vec<(&str, &str, &str, &str)> = result
        .findings
        .iter()
        .map(|f| {
            (
                f.code.code,
                f.property.as_str(),
                f.expected.as_str(),
                f.found.as_str(),
            )
        })
        .collect();
    let mut expected = vec![
        (
            "io.ifc.pset_value_type",
            "Reference",
            "IfcIdentifier",
            "IFCLABEL",
        ),
        (
            "io.ifc.pset_enumeration_item",
            "Status",
            "NEW, EXISTING",
            "OLD",
        ),
        (
            "io.ifc.pset_property_kind",
            "Width",
            "IFCPROPERTYBOUNDEDVALUE",
            "IFCPROPERTYSINGLEVALUE",
        ),
        ("io.ifc.pset_value_type", "Tags", "IfcLabel", ""),
        ("io.ifc.pset_property_unknown", "Colour", "", ""),
    ];
    // The reference type needs the schema tables to judge subtypes.
    if crate::schema::SchemaTables::of(&crate::Schema::Ifc4).is_some() {
        expected.push((
            "io.ifc.pset_value_type",
            "Owner",
            "IfcPerson",
            "IFCORGANIZATION",
        ));
    }
    expected.extend([
        (
            "io.ifc.pset_value_type",
            "Cp.Inner",
            "IfcInteger",
            "IFCREAL",
        ),
        (
            "io.ifc.pset_quantity_kind",
            "Length",
            "IFCQUANTITYLENGTH",
            "IFCQUANTITYAREA",
        ),
        (
            "io.ifc.pset_property_kind",
            "Count",
            "IFCQUANTITYCOUNT",
            "IFCPROPERTYSINGLEVALUE",
        ),
        ("io.ifc.pset_template_unknown", "", "", "Pset_Nope"),
        (
            "io.ifc.pset_set_kind",
            "",
            "IfcElementQuantity",
            "IfcPropertySet",
        ),
    ]);
    assert_eq!(got, expected);
    let error = result.findings[0].error();
    assert_eq!(error.code(), findings::PSET_VALUE_TYPE.code);
    assert!(findings::ALL.contains(&result.findings[0].code));
}

#[test]
fn applicability_is_judged_with_the_schema_tables() {
    // IFC4X3, whose tables every build has.
    let body = format!(
        "{GOOD}#20=IFCSLAB('4YvctVUKr0kugbFTf53O9L',$,$,$,$,$,$,$,$);
#21=IFCWALL('5YvctVUKr0kugbFTf53O9L',$,$,$,$,$,$,$,$);
#22=IFCWALLTYPE('6YvctVUKr0kugbFTf53O9L',$,$,$,$,(#10),$,$,$,.NOTDEFINED.);
#23=IFCRELDEFINESBYPROPERTIES('7YvctVUKr0kugbFTf53O9L',$,$,$,(#20,#21),#10);
"
    );
    let result = check::check(
        &file_in("IFC4X3_ADD2", &body),
        &TEMPLATES,
        &Limits::CONSTRAINED,
        &Cancel::new(),
    )
    .unwrap();
    let findings: Vec<_> = result
        .findings
        .iter()
        .map(|f| (f.code.code, f.object, f.found.as_str()))
        .collect();
    assert_eq!(
        findings,
        [("io.ifc.pset_not_applicable", Some(20), "IFCSLAB")]
    );
}

#[test]
fn limits_bound_the_walk() {
    let limits = Limits {
        record_count: 3,
        ..Limits::CONSTRAINED
    };
    let error = check::check(&file(GOOD), &TEMPLATES, &limits, &Cancel::new()).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(check::check(&file(GOOD), &TEMPLATES, &Limits::CONSTRAINED, &cancel).is_err());
}

#[test]
fn lookups_ignore_case_and_schemas_without_templates_are_not_evaluated() {
    assert!(TEMPLATES.get("PSET_TESTCOMMON").is_some());
    assert!(TEMPLATES.get("Pset_None").is_none());
    let set = TEMPLATES.get("Pset_TestCommon").unwrap();
    assert!(set.property("Status").is_some());
    assert!(set.property("status").is_none());
    assert!(PsetTemplates::of(&crate::Schema::Ifc4x3).is_none());
    let mut f = file(GOOD);
    f.schema = crate::Schema::Ifc4x3;
    let result = f
        .check_property_sets(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    assert_eq!(result.templates, None);
    assert_eq!(result.sets_checked, 0);
}
