//! Malformed and limited input: each error names its code; unresolved references and cycles
//! are reported and the read goes on.

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

use arc_base::error::Value;
use arc_base::{Cancel, Limits};
use arc_io_ifc::loss::Argument;
use arc_io_ifc::{ReadOptions, Schema, read, reasons};

/// A physical file with the given data section, under a valid header.
fn file(data: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn code_of(text: &str, options: &mut ReadOptions<'_>) -> String {
    match read(text.as_bytes(), options) {
        Ok(_) => panic!("read of {text:?} should fail"),
        Err(error) => error.code().to_owned(),
    }
}

#[test]
fn duplicate_instance_name_is_refused() {
    let code = code_of(&file("#1=IFCX();\n#1=IFCY();"), &mut ReadOptions::default());
    assert_eq!(code, "io.ifc.instance_duplicate");
}

#[test]
fn missing_sections_are_refused() {
    let no_header = "ISO-10303-21;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";
    assert_eq!(
        code_of(no_header, &mut ReadOptions::default()),
        "io.ifc.structure_invalid"
    );
    let no_schema =
        "ISO-10303-21;\nHEADER;\nFILE_NAME('a');\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";
    assert_eq!(
        code_of(no_schema, &mut ReadOptions::default()),
        "io.ifc.structure_invalid"
    );
    let no_end = "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\nENDSEC;\n";
    assert_eq!(
        code_of(no_end, &mut ReadOptions::default()),
        "io.ifc.truncated"
    );
}

#[test]
fn unterminated_string_is_truncation() {
    let code = code_of(&file("#1=IFCX('abc"), &mut ReadOptions::default());
    assert_eq!(code, "io.ifc.truncated");
}

#[test]
fn bad_encoded_form_is_refused() {
    let code = code_of(
        &file(r"#1=IFCX('\X2\00E9\X0');"),
        &mut ReadOptions::default(),
    );
    assert_eq!(code, "io.ifc.text_invalid");
}

#[test]
fn list_nesting_over_the_limit_is_refused() {
    let limits = Limits {
        nesting_depth: 3,
        ..Limits::CONSTRAINED
    };
    let code = code_of(
        &file("#1=IFCX((((1))));"),
        &mut ReadOptions::with_limits(limits),
    );
    assert_eq!(code, "io.limit_exceeded");
    let mut options = ReadOptions::with_limits(limits);
    let error = read(file("#1=IFCX((((1))));").as_bytes(), &mut options).unwrap_err();
    let named = error.arguments().get("limit");
    assert_eq!(named, Some(&Value::Text("nesting_depth".to_owned())));
    // Exactly at the limit still reads.
    assert!(
        read(
            file("#1=IFCX(((1)));").as_bytes(),
            &mut ReadOptions::with_limits(limits)
        )
        .is_ok()
    );
}

#[test]
fn too_many_instances_for_a_lowered_limit_are_refused() {
    let limits = Limits {
        record_count: 2,
        ..Limits::CONSTRAINED
    };
    let code = code_of(
        &file("#1=IFCX(1);\n#2=IFCX(2);\n#3=IFCX(3);"),
        &mut ReadOptions::with_limits(limits),
    );
    assert_eq!(code, "io.limit_exceeded");
}

#[test]
fn oversized_input_is_refused_before_parsing() {
    let limits = Limits {
        input_bytes: 16,
        ..Limits::CONSTRAINED
    };
    assert_eq!(
        code_of(&file("#1=IFCX(1);"), &mut ReadOptions::with_limits(limits)),
        "io.limit_exceeded"
    );
}

#[test]
fn cancelled_read_returns_cancelled() {
    let mut options = ReadOptions::default();
    options.cancel.cancel();
    assert_eq!(
        code_of(&file("#1=IFCX(1);"), &mut options),
        "base.cancelled"
    );
}

#[test]
fn unresolved_references_are_kept_and_reported() {
    let output = read(
        file("#1=IFCRELCONTAINEDINSPATIALSTRUCTURE($,$,$,$,(#99),#98);").as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    let reference = output
        .report
        .items
        .iter()
        .find(|i| i.reason == reasons::REFERENCE_UNRESOLVED.code);
    let item = reference.expect("unresolved reference reported");
    assert_eq!(item.arguments.get("count"), Some(&Argument::Int(2)));
    assert_eq!(item.outcome, arc_io_ifc::Outcome::Preserved);
    // The reference stays in the model.
    let parameters = &output.file.instance(1).unwrap().records[0].parameters;
    assert_eq!(parameters[5], arc_io_ifc::Parameter::Reference(98));
    assert_eq!(output.file.schema, Schema::Ifc4);
}

/// The minimal project nodes needed for the spatial tree.
fn spatial(data: &str) -> arc_io_ifc::TypedViews {
    let output = read(file(data).as_bytes(), &mut ReadOptions::default()).unwrap();
    output
        .file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap()
}

#[test]
fn aggregation_cycle_is_cut_and_reported() {
    // Project P contains site S; S contains building B; B contains S again.
    let views = spatial(
        "#1=IFCPROJECT('p',$,'P',$,$,$,$,$,$);\n#2=IFCSITE('s',$,'S',$,$,$,$,$,$,$,$,$,$,$);\n\
#3=IFCBUILDING('b',$,'B',$,$,$,$,$,$,$,$,$);\n#10=IFCRELAGGREGATES('a1',$,$,$,#1,(#2));\n\
#11=IFCRELAGGREGATES('a2',$,$,$,#2,(#3));\n#12=IFCRELAGGREGATES('a3',$,$,$,#3,(#2));",
    );
    assert_eq!(views.spatial.len(), 1);
    let site = &views.spatial[0].children[0];
    assert_eq!(site.children[0].entity.name.as_deref(), Some("B"));
    assert!(site.children[0].children.is_empty());
    assert!(
        views
            .report
            .items
            .iter()
            .any(|i| i.reason == reasons::CYCLE_CUT.code)
    );
    assert!(views.report.has_loss());
}

#[test]
fn cycle_without_a_root_is_reported_and_nothing_hangs() {
    let views = spatial(
        "#2=IFCSITE('s',$,'S',$,$,$,$,$,$,$,$,$,$,$);\n#3=IFCBUILDING('b',$,'B',$,$,$,$,$,$,$,$,$);\n\
#11=IFCRELAGGREGATES('a2',$,$,$,#2,(#3));\n#12=IFCRELAGGREGATES('a3',$,$,$,#3,(#2));",
    );
    assert!(views.spatial.is_empty());
    let cut: i64 = views
        .report
        .items
        .iter()
        .filter(|i| i.reason == reasons::CYCLE_CUT.code)
        .map(|i| match i.arguments.get("count") {
            Some(Argument::Int(count)) => *count,
            _ => 0,
        })
        .sum();
    assert_eq!(cut, 2);
    assert_eq!(views.report.summary.dropped, 2);
}

#[test]
fn nesting_beyond_the_hard_cap_is_refused_whatever_the_profile() {
    // The profile allows any depth; the parser still stops at its own cap of 256 levels.
    let limits = Limits {
        nesting_depth: u32::MAX,
        ..Limits::CONSTRAINED
    };
    let deep = format!("#1=IFCX({}1{});", "(".repeat(1000), ")".repeat(1000));
    let error = read(
        file(&deep).as_bytes(),
        &mut ReadOptions::with_limits(limits),
    )
    .unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    let within = format!("#1=IFCX({}1{});", "(".repeat(200), ")".repeat(200));
    assert!(
        read(
            file(&within).as_bytes(),
            &mut ReadOptions::with_limits(limits)
        )
        .is_ok()
    );
}

#[test]
fn spatial_tree_deeper_than_the_limit_is_refused() {
    // A project, then a chain of 40 spaces, each aggregating the next: 41 levels of tree.
    let mut data = String::from(
        "#1=IFCPROJECT('p',$,'P',$,$,$,$,$,$);\n#100=IFCSPACE('s',$,'S',$,$,$,$,$,$,$,$,$,$,$);\n\
#1000=IFCRELAGGREGATES('r',$,$,$,#1,(#100));\n",
    );
    for i in 1..40u64 {
        data.push_str(&format!(
            "#{}=IFCSPACE('s',$,'S',$,$,$,$,$,$,$,$,$,$,$);\n\
#{}=IFCRELAGGREGATES('r',$,$,$,#{},(#{}));\n",
            100 + i,
            1000 + i,
            99 + i,
            100 + i
        ));
    }
    let output = read(file(&data).as_bytes(), &mut ReadOptions::default()).unwrap();
    let shallow = Limits {
        nesting_depth: 32,
        ..Limits::CONSTRAINED
    };
    let error = output
        .file
        .typed_views(&shallow, &Cancel::new())
        .unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("nesting_depth".to_owned()))
    );
    assert!(
        output
            .file
            .typed_views(&Limits::DEFAULT, &Cancel::new())
            .is_ok()
    );
}
