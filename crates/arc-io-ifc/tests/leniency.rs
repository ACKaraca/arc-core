//! Text, framing and section rules: the encoded forms of strings, the leniency for files in the
//! wild (reported as loss items), and the edition 3 structure of the physical file.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

use arc_io_ifc::loss::Argument;
use arc_io_ifc::{Parameter, ReadOptions, ReadOutput, read, reasons};

/// A physical file with the given sections after the header, followed by the end line.
fn document(sections: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\n{sections}END-ISO-10303-21;\n"
    )
}

/// The start of a physical file up to the data section, so that bytes can be spliced in.
const PREFIX: &str = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n";
/// The end of a physical file after the data section.
const SUFFIX: &str = "\nENDSEC;\nEND-ISO-10303-21;\n";

fn data_file(data: &str) -> String {
    document(&format!("DATA;\n{data}\nENDSEC;\n"))
}

fn read_text(text: &str) -> ReadOutput {
    read(text.as_bytes(), &mut ReadOptions::default()).unwrap()
}

fn code_of(bytes: &[u8]) -> String {
    match read(bytes, &mut ReadOptions::default()) {
        Ok(_) => panic!("read should fail"),
        Err(error) => error.code().to_owned(),
    }
}

fn text_of(literal: &str) -> String {
    let output = read_text(&data_file(&format!("#1=IFCTEXT({literal});")));
    match output.file.instance(1).unwrap().records[0]
        .parameters
        .first()
    {
        Some(Parameter::Text(text)) => text.clone(),
        other => panic!("text expected, got {other:?}"),
    }
}

fn item_count(output: &ReadOutput, reason: &str) -> Option<i64> {
    output
        .report
        .items
        .iter()
        .find(|i| i.reason == reason)
        .map(|i| match i.arguments.get("count") {
            Some(Argument::Int(count)) => *count,
            _ => -1,
        })
}

#[test]
fn x_escape_after_a_page_directive_is_latin1() {
    // \X\hh is U+0000-U+00FF whatever page is selected; only \S\ depends on the page.
    assert_eq!(text_of(r"'\PB\\X\E9'"), "\u{e9}");
    assert_eq!(text_of(r"'\PB\\X2\00E9\X0\'"), "\u{e9}");
}

#[test]
fn x_escape_keeps_the_text_that_follows_it() {
    assert_eq!(text_of(r"'\X\E9a'"), "\u{e9}a");
    assert_eq!(text_of(r"'\X\E9\X\E8'"), "\u{e9}\u{e8}");
}

#[test]
fn raw_tab_carriage_return_and_line_feed_are_kept_and_counted() {
    let output = read_text(&data_file("#1=IFCTEXT('a\tb\r\nc');"));
    assert_eq!(text_of("'a\tb\r\nc'"), "a\tb\r\nc");
    assert_eq!(
        item_count(&output, reasons::TEXT_CONTROL_KEPT.code),
        Some(3)
    );
}

#[test]
fn other_raw_control_bytes_are_still_refused() {
    for control in [0x01u8, 0x00] {
        let mut bytes = format!("{PREFIX}#1=IFCTEXT('a").into_bytes();
        bytes.push(control);
        bytes.extend_from_slice(format!("b');{SUFFIX}").as_bytes());
        assert_eq!(code_of(&bytes), "io.ifc.text_invalid", "{control:#x}");
    }
}

#[test]
fn bytes_that_are_not_utf8_are_read_as_latin1_once_with_a_count() {
    let mut bytes = format!("{PREFIX}#1=IFCTEXT('caf").into_bytes();
    bytes.push(0xE9);
    bytes.extend_from_slice(b"');\n#2=IFCTEXT('");
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    bytes.extend_from_slice(format!("');{SUFFIX}").as_bytes());
    let output = read(&bytes, &mut ReadOptions::default()).unwrap();
    let first = &output.file.instance(1).unwrap().records[0].parameters[0];
    assert_eq!(first, &Parameter::Text("caf\u{e9}".to_owned()));
    let second = &output.file.instance(2).unwrap().records[0].parameters[0];
    assert_eq!(second, &Parameter::Text("\u{ff}\u{fe}".to_owned()));
    assert_eq!(item_count(&output, reasons::TEXT_LATIN1.code), Some(3));
    assert_eq!(
        output
            .report
            .items
            .iter()
            .filter(|i| i.reason == reasons::TEXT_LATIN1.code)
            .count(),
        1
    );
}

#[test]
fn utf8_byte_order_mark_before_the_keyword_is_skipped() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(data_file("#1=IFCX(1);").as_bytes());
    let output = read(&bytes, &mut ReadOptions::default()).unwrap();
    assert!(output.file.instance(1).is_some());
    assert_eq!(item_count(&output, reasons::BYTES_IGNORED.code), Some(3));
}

#[test]
fn padding_after_the_end_is_ignored_and_other_bytes_are_reported() {
    let padded = format!("{}\0\u{1a} \r\n", data_file("#1=IFCX(1);"));
    let output = read_text(&padded);
    assert!(output.file.instance(1).is_some());
    assert_eq!(item_count(&output, reasons::BYTES_IGNORED.code), Some(2));
    assert_eq!(item_count(&output, reasons::TRAILING_BYTES.code), None);

    let trailing = format!("{}XY", data_file("#1=IFCX(1);"));
    let output = read_text(&trailing);
    assert_eq!(item_count(&output, reasons::TRAILING_BYTES.code), Some(2));
}

#[test]
fn decoded_nul_is_refused_as_a_raw_nul_is() {
    for literal in [r"'\X\00'", r"'\X2\0000\X0\'", r"'\X4\00000000\X0\'"] {
        let text = data_file(&format!("#1=IFCTEXT({literal});"));
        assert_eq!(code_of(text.as_bytes()), "io.ifc.text_invalid", "{literal}");
    }
}

#[test]
fn lowercase_e_in_a_real_is_an_error() {
    assert_eq!(
        code_of(data_file("#1=IFCX(1.5e3);").as_bytes()),
        "io.ifc.syntax"
    );
}

#[test]
fn two_data_sections_are_merged_with_or_without_parameters() {
    let text = document("DATA;\n#1=IFCX(1);\nENDSEC;\nDATA('params');\n#2=IFCX(2);\nENDSEC;\n");
    let output = read_text(&text);
    assert!(output.file.instance(1).is_some());
    assert!(output.file.instance(2).is_some());
}

#[test]
fn a_name_repeated_across_data_sections_is_refused() {
    let text = document("DATA;\n#1=IFCX(1);\nENDSEC;\nDATA;\n#1=IFCX(2);\nENDSEC;\n");
    assert_eq!(code_of(text.as_bytes()), "io.ifc.instance_duplicate");
}

#[test]
fn anchor_section_is_skipped_as_a_whole_and_reported() {
    let text = document(
        "ANCHOR;\n#100=IFCX('a;b');\n#101=IFCX(#100);\nENDSEC;\nDATA;\n#1=IFCX(1);\nENDSEC;\n",
    );
    let output = read_text(&text);
    assert!(output.file.instance(1).is_some());
    assert!(output.file.instance(100).is_none());
    let item = output
        .report
        .items
        .iter()
        .find(|i| i.reason == reasons::SECTION_SKIPPED.code)
        .expect("skipped section reported");
    assert_eq!(item.record_kind.as_deref(), Some("ANCHOR"));
    assert_eq!(item.arguments.get("count"), Some(&Argument::Int(1)));
}

#[test]
fn reference_and_signature_sections_are_skipped_too() {
    let text = document(
        "REFERENCE;\n#50=IFCX(1);\nENDSEC;\nDATA;\n#1=IFCX(1);\nENDSEC;\nSIGNATURE;\nENDSEC;\n",
    );
    let output = read_text(&text);
    assert!(output.file.instance(1).is_some());
    assert!(item_count(&output, reasons::SECTION_SKIPPED.code).is_some());
}

#[test]
fn empty_complex_instance_is_a_syntax_error() {
    assert_eq!(code_of(data_file("#1=();").as_bytes()), "io.ifc.syntax");
}

#[test]
fn file_without_a_data_section_is_a_structure_error() {
    let text = document("");
    assert_eq!(code_of(text.as_bytes()), "io.ifc.structure_invalid");
}
