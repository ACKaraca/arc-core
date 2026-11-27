//! Conformance level 3: the domain (`WHERE`) rules of entities and defined types and the global
//! rules. A minimal valid file per schema passes every rule; each mutation violates one rule, which
//! must be reported for the instance concerned with the rule's name in the schema, while nothing
//! else of level 3 fails. The tests need the generated schema tables and skip without them. Every
//! file is written with `ARC_IFC_DUMP` for `tools/ifc-oracle/conformance_compare.py --rules`.

#![allow(clippy::print_stderr, clippy::unwrap_used, reason = "test code")]

mod support;

use arc_base::{Cancel, Limits};
use arc_io_ifc::rules::{self, Rule};
use arc_io_ifc::schema::SchemaTables;
use arc_io_ifc::{Conformance, ReadOptions, RuleScope, Schema, read};

const SCHEMAS: [&str; 3] = ["IFC2X3", "IFC4", "IFC4X3_ADD2"];

/// Valid in all three schemas; the instances the mutations add start at #30.
const BASE: &str = "\
#1=IFCCARTESIANPOINT((0.,0.,0.));
#2=IFCDIRECTION((0.,0.,1.));
#3=IFCAXIS2PLACEMENT3D(#1,$,$);
#4=IFCPROPERTYSINGLEVALUE('Width',$,IFCLENGTHMEASURE(0.2),$);
#5=IFCPROPERTYSET('0YvctVUKr0kugbFTf53O9L',#15,'Pset_Test',$,(#4));
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
#17=IFCCARTESIANPOINT((0.,0.));
#18=IFCRECTANGLEPROFILEDEF(.AREA.,$,#26,1.,2.);
#19=IFCEXTRUDEDAREASOLID(#18,#3,#2,3.);
#20=IFCSHAPEREPRESENTATION(#10,'Body','SweptSolid',(#19));
#21=IFCPRODUCTDEFINITIONSHAPE($,$,(#20));
#22=IFCLOCALPLACEMENT($,#3);
#23=IFCBUILDINGELEMENTPROXY('3Hb4dC2sD0mW1nKxT9z0dP',#15,'X',$,$,#22,#21,$,$);
#24=IFCPOLYLINE((#1,#25));
#25=IFCCARTESIANPOINT((1.,0.,0.));
#26=IFCAXIS2PLACEMENT2D(#17,$);
";

/// One violated rule: the edit of the base and, per schema, the instance and rule expected.
struct Mutation {
    name: &'static str,
    from: &'static str,
    to: &'static str,
    instance: u64,
    rule: Rule,
    /// `Owner.Label` in IFC2X3, IFC4 and IFC4X3; `None` where the schema has no such rule.
    expected: [Option<&'static str>; 3],
}

const END: &str = "#25=IFCCARTESIANPOINT((1.,0.,0.));";

const fn add(
    name: &'static str,
    to: &'static str,
    instance: u64,
    expected: [Option<&'static str>; 3],
) -> Mutation {
    Mutation {
        name,
        from: END,
        to,
        instance,
        rule: rules::WHERE_RULE,
        expected,
    }
}

const fn same(rule: &'static str) -> [Option<&'static str>; 3] {
    [Some(rule), Some(rule), Some(rule)]
}

const MUTATIONS: &[Mutation] = &[
    add(
        "direction-zero",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIRECTION((0.,0.,0.));",
        30,
        [
            None,
            Some("IfcDirection.MagnitudeGreaterZero"),
            Some("IfcDirection.MagnitudeGreaterZero"),
        ],
    ),
    add(
        "point-1d",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCARTESIANPOINT((0.));",
        30,
        [
            Some("IfcCartesianPoint.WR1"),
            Some("IfcCartesianPoint.CP2Dor3D"),
            Some("IfcCartesianPoint.CP2Dor3D"),
        ],
    ),
    Mutation {
        name: "project-without-name",
        from: "'1Xk3kqQ3T1A8E0J5sG9Uy0',#15,'P',",
        to: "'1Xk3kqQ3T1A8E0J5sG9Uy0',#15,$,",
        instance: 8,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcProject.WR31"),
            Some("IfcProject.HasName"),
            Some("IfcProject.HasName"),
        ],
    },
    add(
        "placement-location-2d",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCAXIS2PLACEMENT3D(#17,$,$);",
        30,
        [
            Some("IfcAxis2Placement3D.WR1"),
            Some("IfcAxis2Placement3D.LocationIs3D"),
            Some("IfcAxis2Placement3D.LocationIs3D"),
        ],
    ),
    add(
        "placement-axes-parallel",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCAXIS2PLACEMENT3D(#1,#2,#2);",
        30,
        [
            Some("IfcAxis2Placement3D.WR4"),
            Some("IfcAxis2Placement3D.AxisToRefDirPosition"),
            Some("IfcAxis2Placement3D.AxisToRefDirPosition"),
        ],
    ),
    add(
        "placement-axis-alone",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCAXIS2PLACEMENT3D(#1,#2,$);",
        30,
        [
            Some("IfcAxis2Placement3D.WR5"),
            Some("IfcAxis2Placement3D.AxisAndRefDirProvision"),
            Some("IfcAxis2Placement3D.AxisAndRefDirProvision"),
        ],
    ),
    add(
        "placement-axis-2d",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIRECTION((1.,0.));\n#31=IFCAXIS2PLACEMENT3D(#1,#30,#2);",
        31,
        [
            Some("IfcAxis2Placement3D.WR2"),
            Some("IfcAxis2Placement3D.AxisIs3D"),
            Some("IfcAxis2Placement3D.AxisIs3D"),
        ],
    ),
    add(
        "placement-2d-location-3d",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCAXIS2PLACEMENT2D(#1,$);",
        30,
        [
            Some("IfcAxis2Placement2D.WR2"),
            Some("IfcAxis2Placement2D.LocationIs2D"),
            Some("IfcAxis2Placement2D.LocationIs2D"),
        ],
    ),
    Mutation {
        name: "local-placement-2d-relative",
        from: "#22=IFCLOCALPLACEMENT($,#3);",
        to: "#22=IFCLOCALPLACEMENT(#31,#3);\n#30=IFCAXIS2PLACEMENT2D(#17,$);\n#31=IFCLOCALPLACEMENT($,#30);\n#32=IFCBUILDINGELEMENTPROXY('3Hb4dC2sD0mW1nKxT9z0dQ',#15,'Y',$,$,#31,$,$,$);",
        instance: 22,
        rule: rules::WHERE_RULE,
        expected: same("IfcLocalPlacement.WR21"),
    },
    Mutation {
        name: "two-length-units",
        from: "#9=IFCUNITASSIGNMENT((#7));",
        to: "#9=IFCUNITASSIGNMENT((#7,#30));\n#30=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);",
        instance: 9,
        rule: rules::WHERE_RULE,
        expected: same("IfcUnitAssignment.WR01"),
    },
    Mutation {
        name: "extrusion-in-profile-plane",
        from: "#19=IFCEXTRUDEDAREASOLID(#18,#3,#2,3.);",
        to: "#19=IFCEXTRUDEDAREASOLID(#18,#3,#30,3.);\n#30=IFCDIRECTION((1.,0.,0.));",
        instance: 19,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcExtrudedAreaSolid.WR31"),
            Some("IfcExtrudedAreaSolid.ValidExtrusionDirection"),
            Some("IfcExtrudedAreaSolid.ValidExtrusionDirection"),
        ],
    },
    Mutation {
        name: "swept-curve-profile",
        from: "#18=IFCRECTANGLEPROFILEDEF(.AREA.,",
        to: "#18=IFCRECTANGLEPROFILEDEF(.CURVE.,",
        instance: 19,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcSweptAreaSolid.WR22"),
            Some("IfcSweptAreaSolid.SweptAreaType"),
            Some("IfcSweptAreaSolid.SweptAreaType"),
        ],
    },
    Mutation {
        name: "shape-items-for-another-type",
        from: "'Body','SweptSolid',(#19)",
        to: "'Body','Curve2D',(#19)",
        instance: 20,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcShapeRepresentation.WR24"),
            Some("IfcShapeRepresentation.CorrectItemsForType"),
            Some("IfcShapeRepresentation.CorrectItemsForType"),
        ],
    },
    Mutation {
        name: "shape-without-placement",
        from: "'X',$,$,#22,#21,$,$);",
        to: "'X',$,$,$,#21,$,$);\n#30=IFCBUILDINGELEMENTPROXY('3Hb4dC2sD0mW1nKxT9z0dQ',#15,'Y',$,$,#22,$,$,$);",
        instance: 23,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcProduct.WR1"),
            Some("IfcProduct.PlacementForShapeRepresentation"),
            Some("IfcProduct.PlacementForShapeRepresentation"),
        ],
    },
    Mutation {
        name: "polyline-mixed-dimensions",
        from: "#24=IFCPOLYLINE((#1,#25));",
        to: "#24=IFCPOLYLINE((#1,#17));",
        instance: 24,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcPolyline.WR41"),
            Some("IfcPolyline.SameDim"),
            Some("IfcPolyline.SameDim"),
        ],
    },
    add(
        "polyloop-mixed-dimensions",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCPOLYLOOP((#1,#25,#17));",
        30,
        [
            Some("IfcPolyLoop.WR21"),
            Some("IfcPolyLoop.AllPointsSameDim"),
            Some("IfcPolyLoop.AllPointsSameDim"),
        ],
    ),
    Mutation {
        name: "property-names-repeat",
        from: "'Pset_Test',$,(#4));",
        to: "'Pset_Test',$,(#4,#30));\n#30=IFCPROPERTYSINGLEVALUE('Width',$,IFCLENGTHMEASURE(0.3),$);",
        instance: 5,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcPropertySet.WR32"),
            Some("IfcPropertySet.UniquePropertyNames"),
            Some("IfcPropertySet.UniquePropertyNames"),
        ],
    },
    add(
        "relation-aggregates-itself",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCRELAGGREGATES('2Hb4dC2sD0mW1nKxT9z0dQ',#15,$,$,#23,(#23));",
        30,
        [
            Some("IfcRelDecomposes.WR31"),
            Some("IfcRelAggregates.NoSelfReference"),
            Some("IfcRelAggregates.NoSelfReference"),
        ],
    ),
    add(
        "spatial-element-contained",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCBUILDINGSTOREY('2Hb4dC2sD0mW1nKxT9z0dR',#15,'S',$,$,$,$,$,.ELEMENT.,$);\n#31=IFCRELCONTAINEDINSPATIALSTRUCTURE('2Hb4dC2sD0mW1nKxT9z0dS',#15,$,$,(#30),#30);\n#32=IFCRELAGGREGATES('2Hb4dC2sD0mW1nKxT9z0dT',#15,$,$,#8,(#30));",
        31,
        same("IfcRelContainedInSpatialStructure.WR31"),
    ),
    add(
        "vector-negative-magnitude",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCVECTOR(#2,-1.);",
        30,
        [
            Some("IfcVector.WR1"),
            Some("IfcVector.MagGreaterOrEqualZero"),
            Some("IfcVector.MagGreaterOrEqualZero"),
        ],
    ),
    Mutation {
        name: "person-not-identifiable",
        from: "#11=IFCPERSON($,'Doe',$,$,$,$,$,$);",
        to: "#11=IFCPERSON($,$,$,$,$,$,$,$);",
        instance: 11,
        rule: rules::WHERE_RULE,
        expected: [
            Some("IfcPerson.WR1"),
            Some("IfcPerson.IdentifiablePerson"),
            Some("IfcPerson.IdentifiablePerson"),
        ],
    },
    Mutation {
        name: "owner-history-change-without-date",
        from: ".NOCHANGE.,$,",
        to: ".ADDED.,$,",
        instance: 15,
        rule: rules::WHERE_RULE,
        expected: [
            None,
            Some("IfcOwnerHistory.CorrectChangeAction"),
            Some("IfcOwnerHistory.CorrectChangeAction"),
        ],
    },
    Mutation {
        name: "true-north-3d",
        from: "'Model',3,1.E-05,#3,$);",
        to: "'Model',3,1.E-05,#3,#2);",
        instance: 10,
        rule: rules::WHERE_RULE,
        expected: [
            None,
            Some("IfcGeometricRepresentationContext.North2D"),
            Some("IfcGeometricRepresentationContext.North2D"),
        ],
    },
    add(
        "transformation-axis-2d",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIRECTION((1.,0.));\n#31=IFCCARTESIANTRANSFORMATIONOPERATOR3D(#30,$,#1,$,$);",
        31,
        [
            Some("IfcCartesianTransformationOperator3D.WR2"),
            Some("IfcCartesianTransformationOperator3D.Axis1Is3D"),
            Some("IfcCartesianTransformationOperator3D.Axis1Is3D"),
        ],
    ),
    add(
        "trim-values-of-one-kind",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCAXIS2PLACEMENT2D(#17,$);\n#31=IFCCIRCLE(#30,1.);\n#32=IFCTRIMMEDCURVE(#31,(IFCPARAMETERVALUE(0.),IFCPARAMETERVALUE(1.)),(IFCPARAMETERVALUE(1.)),.T.,.PARAMETER.);",
        32,
        [
            Some("IfcTrimmedCurve.WR41"),
            Some("IfcTrimmedCurve.Trim1ValuesConsistent"),
            Some("IfcTrimmedCurve.Trim1ValuesConsistent"),
        ],
    ),
    add(
        "trim-of-a-bounded-curve",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCTRIMMEDCURVE(#24,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(1.)),.T.,.PARAMETER.);",
        30,
        [
            Some("IfcTrimmedCurve.WR43"),
            Some("IfcTrimmedCurve.NoTrimOfBoundedCurves"),
            Some("IfcTrimmedCurve.NoTrimOfBoundedCurves"),
        ],
    ),
    add(
        "derived-unit-without-type",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDERIVEDUNITELEMENT(#7,2);\n#31=IFCDERIVEDUNIT((#30),.USERDEFINED.,$);",
        31,
        same("IfcDerivedUnit.WR2"),
    ),
    add(
        "derived-unit-single-element",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDERIVEDUNITELEMENT(#7,1);\n#31=IFCDERIVEDUNIT((#30),.LINEARVELOCITYUNIT.,$);",
        31,
        same("IfcDerivedUnit.WR1"),
    ),
    add(
        "conversion-unit-with-wrong-dimensions",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIMENSIONALEXPONENTS(0,1,0,0,0,0,0);\n#31=IFCMEASUREWITHUNIT(IFCLENGTHMEASURE(0.0254),#7);\n#32=IFCCONVERSIONBASEDUNIT(#30,.LENGTHUNIT.,'inch',#31);",
        32,
        same("IfcNamedUnit.WR1"),
    ),
    add(
        "tapered-profiles-of-two-kinds",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCIRCLEPROFILEDEF(.AREA.,$,#26,1.);\n#31=IFCEXTRUDEDAREASOLIDTAPERED(#18,#3,#2,3.,#30);",
        31,
        [
            None,
            Some("IfcExtrudedAreaSolidTapered.CorrectProfileAssignment"),
            Some("IfcExtrudedAreaSolidTapered.CorrectProfileAssignment"),
        ],
    ),
    add(
        "calendar-date-invalid",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCALENDARDATE(31,2,2021);",
        30,
        [Some("IfcCalendarDate.WR21"), None, None],
    ),
    add(
        "assignment-of-the-wrong-object-type",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCGROUP('2Hb4dC2sD0mW1nKxT9z0dU',#15,'G',$,$);\n#31=IFCRELASSIGNSTOGROUP('2Hb4dC2sD0mW1nKxT9z0dV',#15,$,$,(#23),.PROCESS.,#30);",
        31,
        [Some("IfcRelAssigns.WR1"), Some("IfcRelAssigns.WR1"), None],
    ),
    add(
        "indexed-segments-not-consecutive",
        "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCARTESIANPOINTLIST2D(((0.,0.),(1.,0.),(1.,1.)));\n#31=IFCINDEXEDPOLYCURVE(#30,(IFCLINEINDEX((1,2)),IFCLINEINDEX((3,1))),$);",
        31,
        [
            None,
            Some("IfcIndexedPolyCurve.Consecutive"),
            Some("IfcIndexedPolyCurve.Consecutive"),
        ],
    ),
    Mutation {
        name: "profile-negative-width",
        from: "#18=IFCRECTANGLEPROFILEDEF(.AREA.,$,#26,1.,2.);",
        to: "#18=IFCRECTANGLEPROFILEDEF(.AREA.,$,#26,-1.,2.);",
        instance: 18,
        rule: rules::TYPE_RULE,
        expected: same("IfcPositiveLengthMeasure.WR1"),
    },
    Mutation {
        name: "context-dimension-4",
        from: "'Model',3,1.E-05",
        to: "'Model',4,1.E-05",
        instance: 10,
        rule: rules::TYPE_RULE,
        expected: same("IfcDimensionCount.WR1"),
    },
    Mutation {
        name: "colour-ratio-above-one",
        from: END,
        to: "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCOLOURRGB($,1.5,0.,0.);",
        instance: 30,
        rule: rules::TYPE_RULE,
        expected: same("IfcNormalisedRatioMeasure.WR1"),
    },
    Mutation {
        name: "typed-select-value-negative",
        from: "IFCLENGTHMEASURE(0.2)",
        to: "IFCPOSITIVELENGTHMEASURE(-0.2)",
        instance: 4,
        rule: rules::TYPE_RULE,
        expected: same("IfcPositiveLengthMeasure.WR1"),
    },
    Mutation {
        name: "compound-angle-minutes",
        from: "IFCLENGTHMEASURE(0.2)",
        to: "IFCCOMPOUNDPLANEANGLEMEASURE((10,70,0))",
        instance: 4,
        rule: rules::TYPE_RULE,
        expected: [
            Some("IfcCompoundPlaneAngleMeasure.WR2"),
            Some("IfcCompoundPlaneAngleMeasure.MinutesInRange"),
            Some("IfcCompoundPlaneAngleMeasure.MinutesInRange"),
        ],
    },
    Mutation {
        name: "two-projects",
        from: END,
        to: "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCPROJECT('1Xk3kqQ3T1A8E0J5sG9Uy1',#15,'Q',$,$,$,$,(#10),#9);",
        instance: 0,
        rule: rules::GLOBAL_RULE,
        expected: same("IfcSingleProjectInstance.WR1"),
    },
    Mutation {
        name: "contexts-with-different-axes",
        from: END,
        to: "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIRECTION((0.,1.,0.));\n#31=IFCAXIS2PLACEMENT3D(#1,#2,#30);\n#32=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Plan',3,1.E-05,#31,$);",
        instance: 0,
        rule: rules::GLOBAL_RULE,
        expected: same("IfcRepresentationContextSameWCS.WR1"),
    },
];

fn file(schema: &str, body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
FILE_NAME('r.ifc','2026-01-01T00:00:00',(''),(''),'','','');\n\
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

fn level3_failures(result: &Conformance) -> Vec<(u64, String)> {
    result
        .findings
        .iter()
        .filter(|f| f.rule.level == 3)
        .map(|f| (f.instance, f.attribute.clone().unwrap_or_default()))
        .collect()
}

#[test]
fn the_base_file_passes_every_rule() {
    if !tables_present() {
        return;
    }
    for schema in SCHEMAS {
        let text = file(schema, BASE);
        support::dump(&format!("rules-{schema}-base.ifc"), text.as_bytes());
        let result = check(&text);
        assert!(
            result.findings.is_empty(),
            "{schema}: {:?}",
            result.findings
        );
        assert_eq!(result.not_evaluated(), 0, "{schema}");
        for rule in [rules::WHERE_RULE, rules::TYPE_RULE, rules::GLOBAL_RULE] {
            assert!(
                result.count(rule).unwrap().passed > 0,
                "{schema} {}",
                rule.code
            );
        }
        let evaluated = result
            .where_rules
            .iter()
            .filter(|c| c.passed + c.unknown > 0)
            .count();
        assert!(evaluated >= 30, "{schema}: {evaluated}");
        assert!(result.where_rules.iter().all(|c| c.not_evaluated == 0));
    }
}

#[test]
fn every_violated_rule_is_found() {
    if !tables_present() {
        return;
    }
    let mut distinct = std::collections::BTreeSet::new();
    let mut errors = Vec::new();
    for (s, schema) in SCHEMAS.iter().enumerate() {
        for m in MUTATIONS {
            let Some(expected) = m.expected[s] else {
                continue;
            };
            assert_eq!(BASE.matches(m.from).count(), 1, "{}", m.name);
            let mut body = BASE.replacen(m.from, m.to, 1);
            if *schema == "IFC4X3_ADD2" {
                // IFC4X3 adds the optional Name to a derived unit.
                body = body
                    .replace(".USERDEFINED.,$);", ".USERDEFINED.,$,$);")
                    .replace(".LINEARVELOCITYUNIT.,$);", ".LINEARVELOCITYUNIT.,$,$);")
                    // ... and the optional TagList to a point list.
                    .replace("(1.,1.)));", "(1.,1.)),$);");
            }
            let text = file(schema, &body);
            support::dump_nonconformant(&format!("rules-{schema}-{}.ifc", m.name), text.as_bytes());
            let result = check(&text);
            // The mutation breaks only the rule it aims at, and no structure.
            let structural: Vec<_> = result
                .findings
                .iter()
                .filter(|f| f.rule.level < 3)
                .collect();
            let found = level3_failures(&result);
            let rule = result
                .findings
                .iter()
                .find(|f| f.rule.level == 3)
                .map(|f| f.rule);
            if !structural.is_empty()
                || found != [(m.instance, expected.to_owned())]
                || rule != Some(m.rule)
                || result.not_evaluated() != 0
            {
                errors.push(format!(
                    "{schema} {}: {found:?} {structural:?} {} not evaluated",
                    m.name,
                    result.not_evaluated()
                ));
                continue;
            }
            distinct.insert(format!("{schema}:{expected}"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(distinct.len() >= 90, "{}", distinct.len());
}

/// `UNKNOWN` passes and is counted apart: with a two-dimensional axis, the cross product of the
/// placement's axes is `?`, so the comparison of its magnitude is `UNKNOWN` and only the rule on
/// the axis's dimension fails.
#[test]
fn unknown_passes_and_is_counted_apart() {
    if !tables_present() {
        return;
    }
    let text = file(
        "IFC4",
        &BASE.replacen(
            END,
            "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCDIRECTION((1.,0.));\n#31=IFCAXIS2PLACEMENT3D(#1,#30,#25);",
            1,
        ),
    );
    support::dump_nonconformant("rules-IFC4-unknown.ifc", text.as_bytes());
    let result = check(&text);
    assert_eq!(
        level3_failures(&result),
        [(31, "IfcAxis2Placement3D.AxisIs3D".to_owned())]
    );
    let count = result
        .where_rules
        .iter()
        .find(|c| c.name() == "IfcAxis2Placement3D.AxisToRefDirPosition")
        .unwrap();
    assert_eq!(
        (count.unknown, count.failed, count.not_evaluated),
        (1, 0, 0)
    );
    let scopes: Vec<RuleScope> = result.where_rules.iter().map(|c| c.scope).collect();
    for scope in [RuleScope::Entity, RuleScope::Type, RuleScope::Global] {
        assert!(scopes.contains(&scope));
    }
}

/// An instance whose structure is wrong is not evaluated for its rules, never passed.
#[test]
fn rules_of_a_malformed_instance_are_not_evaluated() {
    if !tables_present() {
        return;
    }
    let text = file(
        "IFC4",
        &BASE.replacen(
            "#2=IFCDIRECTION((0.,0.,1.));",
            "#2=IFCDIRECTION((0.,0.,1.),1);",
            1,
        ),
    );
    let result = check(&text);
    let count = result
        .where_rules
        .iter()
        .find(|c| c.name() == "IfcDirection.MagnitudeGreaterZero")
        .unwrap();
    assert_eq!(count.not_evaluated, 1);
    assert_eq!(count.passed, 0);
    assert!(count.reason.as_deref().unwrap().contains("structure"));
    assert!(result.count(rules::WHERE_RULE).unwrap().not_evaluated >= 1);
}

/// Every rule of the three schemas has an expression tree that the interpreter accepts; the
/// counts equal the schema's.
#[test]
fn every_rule_of_every_schema_is_counted() {
    if !tables_present() {
        return;
    }
    for (schema, expected) in SCHEMAS.iter().zip([365, 679, 779]) {
        let result = check(&file(schema, BASE));
        assert_eq!(result.where_rules.len(), expected, "{schema}");
    }
}

/// A cycle of derived attributes (a composite curve whose segment's parent curve is the composite
/// curve itself) stops at the nesting limit: the rules that read the dimension are not evaluated and
/// nothing overflows, with the deepest nesting the large-model profile allows too.
#[test]
fn a_cycle_of_derived_attributes_is_not_evaluated() {
    if !tables_present() {
        return;
    }
    let text = file(
        "IFC4",
        &BASE.replacen(
            END,
            "#25=IFCCARTESIANPOINT((1.,0.,0.));\n#30=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#31);\n#31=IFCCOMPOSITECURVE((#30),.F.);",
            1,
        ),
    );
    let parsed = read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file;
    for limits in [Limits::STRICT, Limits::DEFAULT, Limits::LARGE_MODELS] {
        let result = parsed.conformance(&limits, &Cancel::new()).unwrap();
        let count = result
            .where_rules
            .iter()
            .find(|c| c.name() == "IfcCompositeCurve.SameDim")
            .unwrap();
        assert_eq!(count.not_evaluated, 1);
        assert!(
            count.reason.as_deref().unwrap().contains("depth"),
            "{count:?}"
        );
    }
}
