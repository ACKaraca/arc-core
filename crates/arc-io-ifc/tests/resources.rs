//! Resource bounds of the reader and the views: output amplification, identifiers, the memory and
//! wall-clock estimates, cancellation during and after parsing, nesting and instance names.

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
use arc_io_ifc::{Clock, Progress, ReadOptions, read, reasons};

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

/// A project, a storey and one wall whose name has `length` bytes, listed `repeats` times in a
/// single containment relationship.
fn repeated_wall(length: usize, repeats: usize) -> String {
    let name = "n".repeat(length);
    let listed = vec!["#8"; repeats].join(",");
    file(&format!(
        "#1=IFCPROJECT('p',$,'P',$,$,$,$,$,$);\n#5=IFCBUILDINGSTOREY('s',$,'S',$,$,$,$,$,$,0.);\n\
#8=IFCWALL('w',$,'{name}',$,$,$,$,$,$);\n#10=IFCRELAGGREGATES('a',$,$,$,#1,(#5));\n\
#14=IFCRELCONTAINEDINSPATIALSTRUCTURE('r',$,$,$,({listed}),#5);"
    ))
}

/// The count of the one loss item with the given reason, or 0.
fn count_of(items: &[arc_io_ifc::LossItem], reason: &str) -> i64 {
    items
        .iter()
        .filter(|item| item.reason == reason)
        .map(|item| match item.arguments.get("count") {
            Some(Argument::Int(count)) => *count,
            _ => 0,
        })
        .sum()
}

#[test]
fn repeated_element_is_materialized_once_and_its_repeats_counted() {
    let output = read(
        repeated_wall(300, 500).as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    // The budget left for the views is the file's parsed estimate plus 24 KiB: the inverse index
    // of the 500 references (32 bytes each) and one 300-byte wall with its one reference, far from
    // enough for 500 copies of the wall.
    let limits = Limits {
        memory_bytes: Some(output.file.parsed_bytes() + 24 * 1024),
        ..Limits::CONSTRAINED
    };
    let views = output
        .file
        .typed_views(&limits, &Cancel::new())
        .expect("views of one distinct element fit the budget");
    let contained = &views.spatial[0].children[0].contained;
    assert_eq!(contained.len(), 1);
    assert_eq!(contained[0].name.as_ref().map(String::len), Some(300));
    assert_eq!(
        count_of(&views.report.items, reasons::DUPLICATE_PLACEMENT.code),
        499
    );
    assert_eq!(views.report.summary.dropped, 499);
}

#[test]
fn views_memory_limit_is_enforced() {
    let output = read(
        repeated_wall(300, 2).as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    let limits = Limits {
        memory_bytes: Some(output.file.parsed_bytes() + 300),
        ..Limits::CONSTRAINED
    };
    let error = output
        .file
        .typed_views(&limits, &Cancel::new())
        .unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("memory_bytes".to_owned()))
    );
}

#[test]
fn record_count_limit_applies_to_emitted_entries() {
    let output = read(repeated_wall(8, 3).as_bytes(), &mut ReadOptions::default()).unwrap();
    let limits = Limits {
        record_count: 2,
        ..Limits::CONSTRAINED
    };
    let error = output
        .file
        .typed_views(&limits, &Cancel::new())
        .unwrap_err();
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("record_count".to_owned()))
    );
}

#[test]
fn identifier_longer_than_255_bytes_is_refused() {
    let long = format!("#1=IFC{};", "A".repeat(253));
    assert_eq!(
        code_of(&file(&long), &mut ReadOptions::default()),
        "io.ifc.syntax"
    );
}

#[test]
fn identifier_with_a_dash_is_refused() {
    assert_eq!(
        code_of(&file("#1=IFC-X(1);"), &mut ReadOptions::default()),
        "io.ifc.syntax"
    );
}

#[test]
fn user_defined_keyword_is_kept_with_its_bang() {
    let output = read(
        file("#1=!USERDEF(1);").as_bytes(),
        &mut ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        output.file.instance(1).unwrap().records[0].type_name,
        "!USERDEF"
    );
}

#[test]
fn control_byte_in_a_type_name_is_refused() {
    assert_eq!(
        code_of(&file("#1=IFCX\u{7}(1);"), &mut ReadOptions::default()),
        "io.ifc.syntax"
    );
}

#[test]
fn thousand_unresolved_references_in_one_record_are_one_item() {
    let refs: Vec<String> = (2..1002).map(|n| format!("#{n}")).collect();
    let data = format!(
        "#1=IFCRELCONTAINEDINSPATIALSTRUCTURE($,$,$,$,({}),$);",
        refs.join(",")
    );
    let output = read(file(&data).as_bytes(), &mut ReadOptions::default()).unwrap();
    let items: Vec<_> = output
        .report
        .items
        .iter()
        .filter(|i| i.reason == reasons::REFERENCE_UNRESOLVED.code)
        .collect();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].arguments.get("count"), Some(&Argument::Int(1000)));
}

#[test]
fn many_dollar_parameters_exceed_a_lowered_memory_limit() {
    let dollars = vec!["$"; 300].join(",");
    let limits = Limits {
        memory_bytes: Some(4096),
        ..Limits::CONSTRAINED
    };
    let text = file(&format!("#1=IFCX({dollars});"));
    let error = read(text.as_bytes(), &mut ReadOptions::with_limits(limits)).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("memory_bytes".to_owned()))
    );
}

/// A clock that reads zero at the start of the read and five seconds later.
struct JumpClock {
    calls: u32,
}

impl Clock for JumpClock {
    fn millis(&mut self) -> u64 {
        self.calls += 1;
        if self.calls == 1 { 0 } else { 5_000 }
    }
}

#[test]
fn wall_clock_limit_uses_the_injected_clock() {
    let limits = Limits {
        wall_clock_seconds: Some(1),
        ..Limits::CONSTRAINED
    };
    let mut options = ReadOptions {
        limits,
        clock: Some(Box::new(JumpClock { calls: 0 })),
        ..ReadOptions::default()
    };
    let error = read(file("#1=IFCX(1);").as_bytes(), &mut options).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("wall_clock_seconds".to_owned()))
    );
}

#[test]
fn cancellation_during_parsing_stops_the_read() {
    let cancel = Cancel::new();
    let token = cancel.clone();
    let mut progress = |_: Progress| token.cancel();
    let mut options = ReadOptions {
        cancel,
        progress: Some(&mut progress),
        ..ReadOptions::default()
    };
    assert_eq!(
        code_of(&file("#1=IFCX(1,2,3);"), &mut options),
        "base.cancelled"
    );
}

#[test]
fn cancellation_after_parsing_stops_the_read() {
    // The last progress report is the end of the transfer, after parsing and before the report.
    let cancel = Cancel::new();
    let token = cancel.clone();
    let mut progress = |p: Progress| {
        if p.done == p.total {
            token.cancel();
        }
    };
    let mut options = ReadOptions {
        cancel,
        progress: Some(&mut progress),
        ..ReadOptions::default()
    };
    assert_eq!(
        code_of(&file("#1=IFCX(1);"), &mut options),
        "base.cancelled"
    );
}

#[test]
fn cancelled_token_stops_typed_views() {
    let output = read(file("#1=IFCX(1);").as_bytes(), &mut ReadOptions::default()).unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    let error = output
        .file
        .typed_views(&Limits::CONSTRAINED, &cancel)
        .unwrap_err();
    assert_eq!(error.code(), "base.cancelled");
}

#[test]
fn typed_in_typed_nesting_is_refused_one_level_past_the_limit() {
    let limits = Limits {
        nesting_depth: 3,
        ..Limits::CONSTRAINED
    };
    assert!(
        read(
            file("#1=IFCX(IFCA(IFCB(1)));").as_bytes(),
            &mut ReadOptions::with_limits(limits)
        )
        .is_ok()
    );
    let error = read(
        file("#1=IFCX(IFCA(IFCB(IFCC(1))));").as_bytes(),
        &mut ReadOptions::with_limits(limits),
    )
    .unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&Value::Text("nesting_depth".to_owned()))
    );
}

#[test]
fn largest_instance_name_is_read_and_placed() {
    let data = "#18446744073709551615=IFCPROJECT('p',$,'P',$,$,$,$,$,$);";
    let output = read(file(data).as_bytes(), &mut ReadOptions::default()).unwrap();
    assert!(output.file.instance(u64::MAX).is_some());
    let views = output
        .file
        .typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    assert_eq!(views.spatial[0].entity.instance, u64::MAX);
    // One past the largest name is not a name the grammar accepts.
    assert_eq!(
        code_of(
            &file("#18446744073709551616=IFCX(1);"),
            &mut ReadOptions::default()
        ),
        "io.ifc.value_invalid"
    );
}
