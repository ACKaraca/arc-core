//! The physical file grammar: parameter kinds, string encodings, comments, white space, complex
//! instances, forward references and case-insensitive entity names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

use arc_io_ifc::{Parameter, ReadOptions, read, reasons};

/// A physical file with the given data section, under a valid header.
fn file(data: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('x'),'2;1');\nFILE_NAME('a.ifc','',(''),(''),'','','');\n\
FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn parameters(data: &str) -> Vec<Parameter> {
    let output = read(file(data).as_bytes(), &mut ReadOptions::default()).unwrap();
    output.file.instance(1).unwrap().records[0]
        .parameters
        .clone()
}

fn text_of(literal: &str) -> String {
    let parsed = parameters(&format!("#1=IFCTEXT({literal});"));
    match parsed.first() {
        Some(Parameter::Text(text)) => text.clone(),
        other => panic!("text expected, got {other:?}"),
    }
}

#[test]
fn every_parameter_kind_parses_to_its_value() {
    let parsed = parameters(
        "#1=IFCTEST(42,-7,+3,1.5,-.5,2.E3,2.0E-3,'abc',.T.,.F.,.U.,.ELEMENT_NAME.,\"0A3F\",#5,$,*,\
IFCLABEL('x'),((1,2),(3)),());",
    );
    assert_eq!(parsed[0], Parameter::Integer(42));
    assert_eq!(parsed[1], Parameter::Integer(-7));
    assert_eq!(parsed[2], Parameter::Integer(3));
    assert_eq!(parsed[3], Parameter::Real(1.5));
    assert_eq!(parsed[4], Parameter::Real(-0.5));
    assert_eq!(parsed[5], Parameter::Real(2000.0));
    assert_eq!(parsed[6], Parameter::Real(0.002));
    assert_eq!(parsed[7], Parameter::Text("abc".to_owned()));
    assert_eq!(parsed[8], Parameter::Enumeration("T".to_owned()));
    assert_eq!(parsed[9], Parameter::Enumeration("F".to_owned()));
    assert_eq!(parsed[10], Parameter::Enumeration("U".to_owned()));
    assert_eq!(
        parsed[11],
        Parameter::Enumeration("ELEMENT_NAME".to_owned())
    );
    assert_eq!(parsed[12], Parameter::Binary("0A3F".to_owned()));
    assert_eq!(parsed[13], Parameter::Reference(5));
    assert_eq!(parsed[14], Parameter::Omitted);
    assert_eq!(parsed[15], Parameter::Derived);
    assert_eq!(
        parsed[16],
        Parameter::Typed {
            type_name: "IFCLABEL".to_owned(),
            value: Box::new(Parameter::Text("x".to_owned())),
        }
    );
    assert_eq!(
        parsed[17],
        Parameter::List(vec![
            Parameter::List(vec![Parameter::Integer(1), Parameter::Integer(2)]),
            Parameter::List(vec![Parameter::Integer(3)]),
        ])
    );
    assert_eq!(parsed[18], Parameter::List(Vec::new()));
}

#[test]
fn doubled_apostrophe_and_doubled_backslash() {
    assert_eq!(text_of("'it''s'"), "it's");
    assert_eq!(text_of(r"'back\\slash'"), r"back\slash");
}

#[test]
fn encoded_forms_decode_to_utf8() {
    // \S\c: the upper half of ISO 8859-1; \X\hh: one byte of the same page.
    assert_eq!(text_of(r"'\S\A'"), "\u{c1}");
    assert_eq!(text_of(r"'\X\E9'"), "\u{e9}");
    // \PA\ selects the Latin-1 page explicitly; \X\ then reads the same.
    assert_eq!(text_of(r"'\PA\\X\E9'"), "\u{e9}");
    // \X2\: UTF-16 code units, and a surrogate pair.
    assert_eq!(text_of(r"'\X2\00E900FC\X0\'"), "\u{e9}\u{fc}");
    assert_eq!(text_of(r"'\X2\D83DDE00\X0\'"), "\u{1f600}");
    // \X4\: code points.
    assert_eq!(text_of(r"'\X4\0001F600\X0\'"), "\u{1f600}");
    // UTF-8 bytes in the text are kept as they are.
    assert_eq!(text_of("'caf\u{e9}'"), "caf\u{e9}");
}

#[test]
fn comments_and_white_space_between_tokens() {
    let text = "ISO-10303-21 /* lead */ ;\r\n HEADER ;\n FILE_SCHEMA ( ( 'IFC4' ) ) ; /* c */ ENDSEC;\n\
DATA;\n#1 /*c*/ = /*d*/ IFCX /**/ ( 1 , /* two */ 2 ) ;\n\t#2=IFCY('a /* not a comment */ b');\n\
ENDSEC;\nEND-ISO-10303-21;\n";
    let output = read(text.as_bytes(), &mut ReadOptions::default()).unwrap();
    assert_eq!(output.file.instances().len(), 2);
    let first = &output.file.instance(1).unwrap().records[0];
    assert_eq!(first.type_name, "IFCX");
    assert_eq!(
        first.parameters,
        [Parameter::Integer(1), Parameter::Integer(2)]
    );
    let second = &output.file.instance(2).unwrap().records[0];
    assert_eq!(
        second.parameters,
        [Parameter::Text("a /* not a comment */ b".to_owned())]
    );
}

#[test]
fn complex_instance_keeps_its_records() {
    let output = read(
        file("#7=(IFCA(1)IFCB('x',2));").as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    let instance = output.file.instance(7).unwrap();
    let types: Vec<_> = instance
        .records
        .iter()
        .map(|r| r.type_name.as_str())
        .collect();
    assert_eq!(types, ["IFCA", "IFCB"]);
    assert_eq!(instance.records[1].parameters.len(), 2);
}

#[test]
fn forward_references_resolve_and_entity_names_are_upper_cased() {
    let output = read(
        file("#1=ifcwall(#2);\n#2=IFCY(1);").as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        output.file.instance(1).unwrap().records[0].type_name,
        "IFCWALL"
    );
    assert_eq!(
        output.file.instance(1).unwrap().records[0].parameters,
        [Parameter::Reference(2)]
    );
    assert!(
        !output
            .report
            .items
            .iter()
            .any(|i| i.reason == reasons::REFERENCE_UNRESOLVED.code)
    );
}

#[test]
fn real_without_point_and_overflowing_integer_are_refused() {
    for data in [
        "#1=IFCX(1E5);",
        "#1=IFCX(99999999999999999999);",
        "#1=IFCX(.ABC);",
    ] {
        let error = read(file(data).as_bytes(), &mut ReadOptions::default()).unwrap_err();
        assert_eq!(error.code(), "io.ifc.value_invalid", "{data}");
    }
}
