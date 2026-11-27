//! The conformance checker, levels 1 and 2: a minimal valid file per schema has no finding, and one
//! structural error of each kind injected into it is found at the instance and attribute concerned.
//! The tests need the generated schema tables and skip without them, except the check that nothing
//! is counted as passed then. Every mutated file is written with `ARC_IFC_DUMP` for
//! `tools/ifc-oracle/conformance_compare.py`.

#![allow(clippy::print_stderr, clippy::unwrap_used, reason = "test code")]

mod support;

use arc_base::{Cancel, Limits};
use arc_io_ifc::rules::{self, Rule};
use arc_io_ifc::schema::SchemaTables;
use arc_io_ifc::{Conformance, ReadOptions, Schema, read};

const SCHEMAS: [(&str, Schema); 3] = [
    ("IFC2X3", Schema::Ifc2x3),
    ("IFC4", Schema::Ifc4),
    ("IFC4X3_ADD2", Schema::Ifc4x3),
];

/// A small file whose instances have the same layout in IFC2X3, IFC4 and IFC4X3 and that passes
/// levels 1 and 2 in each.
const BASE: &str = "\
#1=IFCCARTESIANPOINT((0.,0.,0.));
#2=IFCDIRECTION((0.,0.,1.));
#3=IFCAXIS2PLACEMENT3D(#1,$,$);
#4=IFCPROPERTYSINGLEVALUE('Width',$,IFCLENGTHMEASURE(0.2),$);
#5=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L',#15,'Pset_Test',$,(#4));
#6=IFCDIMENSIONALEXPONENTS(0,0,0,0,0,0,0);
#7=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);
#8=IFCPROJECT('1Xk3kqQ3T1A8E0J5sG9Uy0',#15,'P',$,$,$,$,(#10),#9);
#9=IFCUNITASSIGNMENT((#7));
#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#3,$);
#11=IFCPERSON($,'Doe',$,$,$,$,$,$);
#12=IFCORGANIZATION($,'Org',$,$,$);
#13=IFCPERSONANDORGANIZATION(#11,#12,$);
#14=IFCAPPLICATION(#12,'1','App','App');
#15=IFCOWNERHISTORY(#13,#14,$,.NOCHANGE.,$,$,$,0);
#16=IFCRELDEFINESBYPROPERTIES('2Hb4dC2sD0mW1nKxT9z0dP',#15,$,$,(#8),#5);
";

fn file(schema: &str, body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
FILE_NAME('c.ifc','2026-01-01T00:00:00',(''),(''),'','','');\n\
FILE_SCHEMA(('{schema}'));\nENDSEC;\nDATA;\n{body}ENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn check(text: &str) -> Conformance {
    read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file
        .conformance(&Limits::DEFAULT, &Cancel::new())
        .unwrap()
}

fn tables_present() -> bool {
    let present = SchemaTables::of(&Schema::Ifc4).is_some();
    if !present {
        eprintln!("skipped: schema tables not generated (cargo xtask ifc-schema)");
    }
    present
}

/// The kinds of structural error: a name, the edit of the base, and the instance, attribute and
/// rule it must be reported as.
struct Mutation {
    name: &'static str,
    from: &'static str,
    to: &'static str,
    instance: u64,
    attribute: Option<&'static str>,
    rule: Rule,
}

const MUTATIONS: &[Mutation] = &[
    Mutation {
        name: "entity-unknown",
        from: "#16=",
        to: "#20=IFCNOTANENTITY(1);\n#16=",
        instance: 20,
        attribute: None,
        rule: rules::ENTITY_UNKNOWN,
    },
    Mutation {
        name: "entity-abstract",
        from: "#16=",
        to: "#20=IFCNAMEDUNIT(#6,.LENGTHUNIT.);\n#16=",
        instance: 20,
        attribute: None,
        rule: rules::ENTITY_ABSTRACT,
    },
    Mutation {
        name: "complex-invalid",
        from: "#16=",
        to: "#20=(IFCCARTESIANPOINT((0.,0.))IFCDIRECTION((1.,0.)));\n#16=",
        instance: 20,
        attribute: None,
        rule: rules::COMPLEX_INVALID,
    },
    Mutation {
        name: "attribute-count",
        from: "#1=IFCCARTESIANPOINT((0.,0.,0.));",
        to: "#1=IFCCARTESIANPOINT((0.,0.,0.),5);",
        instance: 1,
        attribute: None,
        rule: rules::ATTRIBUTE_COUNT,
    },
    Mutation {
        name: "value-missing",
        from: "#4=IFCPROPERTYSINGLEVALUE('Width',",
        to: "#4=IFCPROPERTYSINGLEVALUE($,",
        instance: 4,
        attribute: Some("Name"),
        rule: rules::VALUE_MISSING,
    },
    Mutation {
        name: "derived-omitted",
        from: "#7=IFCSIUNIT(*,",
        to: "#7=IFCSIUNIT($,",
        instance: 7,
        attribute: Some("Dimensions"),
        rule: rules::DERIVED_MISPLACED,
    },
    Mutation {
        name: "derived-not-derived",
        from: "#4=IFCPROPERTYSINGLEVALUE('Width',$,",
        to: "#4=IFCPROPERTYSINGLEVALUE('Width',*,",
        instance: 4,
        attribute: None,
        rule: rules::DERIVED_MISPLACED,
    },
    Mutation {
        name: "value-type",
        from: "#4=IFCPROPERTYSINGLEVALUE('Width',",
        to: "#4=IFCPROPERTYSINGLEVALUE(5,",
        instance: 4,
        attribute: Some("Name"),
        rule: rules::VALUE_TYPE,
    },
    Mutation {
        name: "value-type-real-for-integer",
        from: "#6=IFCDIMENSIONALEXPONENTS(0,",
        to: "#6=IFCDIMENSIONALEXPONENTS(0.,",
        instance: 6,
        attribute: Some("LengthExponent"),
        rule: rules::VALUE_TYPE,
    },
    Mutation {
        name: "value-type-typed-outside-select",
        from: "1.E-05,#3,$);",
        to: "IFCREAL(1.E-05),#3,$);",
        instance: 10,
        attribute: Some("Precision"),
        rule: rules::VALUE_TYPE,
    },
    Mutation {
        name: "value-type-boolean",
        from: "IFCLENGTHMEASURE(0.2)",
        to: "IFCBOOLEAN(.U.)",
        instance: 4,
        attribute: Some("NominalValue"),
        rule: rules::VALUE_TYPE,
    },
    Mutation {
        name: "enumeration-item",
        from: ".LENGTHUNIT.,$,.METRE.",
        to: ".LENGTHUNIT.,$,.FURLONG.",
        instance: 7,
        attribute: Some("Name"),
        rule: rules::ENUMERATION_ITEM,
    },
    Mutation {
        name: "select-member-typed",
        from: "IFCLENGTHMEASURE(0.2)",
        to: "IFCUNITENUM(.LENGTHUNIT.)",
        instance: 4,
        attribute: Some("NominalValue"),
        rule: rules::SELECT_MEMBER,
    },
    Mutation {
        name: "select-member-untyped",
        from: "IFCLENGTHMEASURE(0.2)",
        to: "0.2",
        instance: 4,
        attribute: Some("NominalValue"),
        rule: rules::SELECT_MEMBER,
    },
    Mutation {
        name: "select-member-entity",
        from: "IFCLENGTHMEASURE(0.2),$);",
        to: "IFCLENGTHMEASURE(0.2),#1);",
        instance: 4,
        attribute: Some("Unit"),
        rule: rules::SELECT_MEMBER,
    },
    Mutation {
        name: "reference-type",
        from: "#3=IFCAXIS2PLACEMENT3D(#1,",
        to: "#3=IFCAXIS2PLACEMENT3D(#2,",
        instance: 3,
        attribute: Some("Location"),
        rule: rules::REFERENCE_TYPE,
    },
    Mutation {
        name: "reference-dangling",
        from: "#3=IFCAXIS2PLACEMENT3D(#1,",
        to: "#3=IFCAXIS2PLACEMENT3D(#99,",
        instance: 3,
        attribute: Some("Location"),
        rule: rules::REFERENCE_DANGLING,
    },
    Mutation {
        name: "aggregate-size",
        from: "'Pset_Test',$,(#4));",
        to: "'Pset_Test',$,());",
        instance: 5,
        attribute: Some("HasProperties"),
        rule: rules::AGGREGATE_SIZE,
    },
    Mutation {
        name: "aggregate-size-nested",
        from: "#9=IFCUNITASSIGNMENT((#7));\n",
        to: "#9=IFCUNITASSIGNMENT((#7));\n#21=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.)));\n",
        instance: 21,
        attribute: Some("CoordList"),
        rule: rules::AGGREGATE_SIZE,
    },
    Mutation {
        name: "aggregate-unique",
        from: "'Pset_Test',$,(#4));",
        to: "'Pset_Test',$,(#4,#4));",
        instance: 5,
        attribute: Some("HasProperties"),
        rule: rules::AGGREGATE_UNIQUE,
    },
    Mutation {
        name: "value-width",
        from: "#5=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L'",
        to: "#5=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9LX'",
        instance: 5,
        attribute: Some("GlobalId"),
        rule: rules::VALUE_WIDTH,
    },
    Mutation {
        name: "inverse-upper",
        from: "#16=",
        to: "#17=IFCSITE('3Gf5dE3tE1nX2pLyU0A1eQ',#15,'S',$,$,$,$,$,.ELEMENT.,$,$,$,$,$);\n\
#18=IFCRELAGGREGATES('0Hg6eF4uF2oY3qMzV1B2fR',#15,$,$,#8,(#17));\n\
#19=IFCRELAGGREGATES('1Jh7fG5vG3pZ4rNaW2C3gS',#15,$,$,#8,(#17));\n#16=",
        instance: 17,
        attribute: Some("Decomposes"),
        rule: rules::INVERSE_CARDINALITY,
    },
    Mutation {
        name: "inverse-lower",
        from: "#16=",
        to: "#22=IFCPRODUCTDEFINITIONSHAPE($,$,(#23));\n\
#23=IFCSHAPEREPRESENTATION(#10,'Body','PointCloud',(#1));\n#16=",
        instance: 22,
        attribute: Some("ShapeOfProduct"),
        rule: rules::INVERSE_CARDINALITY,
    },
    Mutation {
        name: "unique-rule",
        from: "#5=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L'",
        to: "#5=IFCPROPERTYSET('1Xk3kqQ3T1A8E0J5sG9Uy0'",
        instance: 8,
        attribute: Some("GlobalId"),
        rule: rules::UNIQUE_RULE,
    },
];

/// Point lists exist from IFC4 on.
fn applies(m: &Mutation, schema: &Schema) -> bool {
    !(m.name == "aggregate-size-nested" && *schema == Schema::Ifc2x3)
}

#[test]
fn the_base_file_passes_in_every_schema() {
    if !tables_present() {
        return;
    }
    for (identifier, _) in SCHEMAS {
        let text = file(identifier, BASE);
        support::dump(
            &format!("conformance-{identifier}-base.ifc"),
            text.as_bytes(),
        );
        let result = check(&text);
        assert!(
            result.findings.is_empty(),
            "{identifier}: {:?}",
            result.findings
        );
        assert_eq!(result.not_evaluated(), 0, "{identifier}");
        assert!(
            result.rules.iter().filter(|c| c.passed > 0).count() >= 12,
            "{identifier}: {:?}",
            result.rules
        );
    }
}

#[test]
fn every_injected_error_is_found() {
    if !tables_present() {
        return;
    }
    for (identifier, schema) in SCHEMAS {
        for m in MUTATIONS.iter().filter(|m| applies(m, &schema)) {
            assert_eq!(BASE.matches(m.from).count(), 1, "{}", m.name);
            let mut body = BASE.replacen(m.from, m.to, 1);
            if schema == Schema::Ifc4x3 && m.name == "aggregate-size-nested" {
                // IFC4X3 adds the optional TagList to a point list.
                body = body.replace("(1.,0.)));", "(1.,0.)),$);");
            }
            let text = file(identifier, &body);
            support::dump_nonconformant(
                &format!("conformance-{identifier}-{}.ifc", m.name),
                text.as_bytes(),
            );
            assert!(
                read(text.as_bytes(), &mut ReadOptions::default()).is_ok(),
                "{identifier} {}",
                m.name
            );
            let result = check(&text);
            let hit = result.findings.iter().any(|f| {
                f.rule == m.rule
                    && f.instance == m.instance
                    && (m.attribute.is_none() || f.attribute.as_deref() == m.attribute)
            });
            assert!(hit, "{identifier} {}: {:?}", m.name, result.findings);
            assert!(result.count(m.rule).unwrap().failed >= 1);
        }
    }
}

/// An integer where a real is declared is a real number, as EXPRESS makes integers reals.
#[test]
fn an_integer_is_a_real() {
    if !tables_present() {
        return;
    }
    let text = file("IFC4", &BASE.replace("((0.,0.,0.))", "((0,0,0))"));
    assert!(check(&text).findings.is_empty());
}

/// A wrong parameter count leaves the positions unknown: the value rules of that instance are not
/// evaluated, never passed.
#[test]
fn rules_that_cannot_be_decided_are_not_evaluated() {
    if !tables_present() {
        return;
    }
    let text = file(
        "IFC4",
        &BASE.replacen(
            "#1=IFCCARTESIANPOINT((0.,0.,0.));",
            "#1=IFCCARTESIANPOINT();",
            1,
        ),
    );
    let result = check(&text);
    assert_eq!(result.count(rules::ATTRIBUTE_COUNT).unwrap().failed, 1);
    assert_eq!(result.count(rules::VALUE_TYPE).unwrap().not_evaluated, 1);

    let text = file(
        "IFC4",
        &BASE.replacen("#16=", "#20=IFCNOTANENTITY(1);\n#16=", 1),
    );
    let result = check(&text);
    assert_eq!(
        result.count(rules::ENTITY_ABSTRACT).unwrap().not_evaluated,
        1
    );
    assert_eq!(result.count(rules::UNIQUE_RULE).unwrap().not_evaluated, 1);
    assert_eq!(
        result
            .count(rules::INVERSE_CARDINALITY)
            .unwrap()
            .not_evaluated,
        1
    );
}

/// Without tables (a schema the reader does not know, or a build without the generated tables)
/// every rule is not evaluated for every instance and nothing passes.
#[test]
fn without_tables_nothing_passes() {
    let result = check(&file("IFC9", BASE));
    assert!(result.schema.is_none());
    assert!(result.findings.is_empty());
    for count in &result.rules {
        assert_eq!(
            (count.passed, count.failed, count.not_evaluated),
            (0, 0, 16)
        );
    }
}

/// Findings are kept to the bound while the counts stay exact.
#[test]
fn findings_are_bounded() {
    if !tables_present() {
        return;
    }
    let mut body = String::from(BASE);
    for i in 100..130 {
        body.push_str(&format!("#{i}=IFCCARTESIANPOINT((0.,0.,0.,0.));\n"));
    }
    let text = file("IFC4", &body);
    let limits = Limits {
        record_count: 10,
        ..Limits::DEFAULT
    };
    let read_limits = ReadOptions::default();
    let parsed = read(text.as_bytes(), &mut { read_limits }).unwrap().file;
    let result = parsed.conformance(&limits, &Cancel::new());
    // The inverse index alone has more entries than the limit.
    assert!(result.is_err());
    let limits = Limits {
        record_count: 1000,
        ..Limits::DEFAULT
    };
    let result = parsed.conformance(&limits, &Cancel::new()).unwrap();
    assert_eq!(result.count(rules::AGGREGATE_SIZE).unwrap().failed, 30);
    assert_eq!(result.findings.len(), 30);
    assert!(!result.truncated);
}

#[test]
fn cancellation_stops_the_check() {
    let cancel = Cancel::new();
    cancel.cancel();
    let parsed = read(file("IFC4", BASE).as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file;
    if parsed.tables().is_some() {
        assert!(parsed.conformance(&Limits::DEFAULT, &cancel).is_err());
    }
}

/// A complex instance whose parts are an entity and its supertype is valid; each part holds the
/// attributes its entity declares, with `*` where another part re-declares one as derived.
#[test]
fn a_valid_complex_instance_passes() {
    if !tables_present() {
        return;
    }
    for (identifier, _) in SCHEMAS {
        let body = BASE.replacen(
            "#16=",
            "#20=(IFCNAMEDUNIT(*,.LENGTHUNIT.)IFCSIUNIT($,.METRE.));\n#16=",
            1,
        );
        let text = file(identifier, &body);
        support::dump_nonconformant(
            &format!("conformance-{identifier}-complex-valid.ifc"),
            text.as_bytes(),
        );
        let result = check(&text);
        assert!(
            result.findings.is_empty(),
            "{identifier}: {:?}",
            result.findings
        );
        assert_eq!(result.count(rules::COMPLEX_INVALID).unwrap().passed, 1);
    }
}
