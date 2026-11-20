//! Stored header variables are checked before the export writes them back: derived variables are
//! omitted, handles must name an object the file keeps, names must name a table entry, values
//! must fit their group codes, and the code page is not written for the UTF-8 output.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_draft_dxf::plan::{FOREIGN_HEADER, decode_record};
use arc_draft_dxf::{HandlePolicy, ImportOptions, export_drawing, export_drawing_with, plan};
use arc_io_dxf::loss::{Argument, LossItem, LossReport};
use arc_io_dxf::model::HeaderVariable;
use arc_io_dxf::{Group, Handle, Value};
use support::{Env, bytes, circle, drawing};

fn var(name: &str, values: Vec<Group>) -> HeaderVariable {
    HeaderVariable {
        name: name.to_owned(),
        values,
    }
}

fn names(header: &[HeaderVariable]) -> Vec<&str> {
    header.iter().map(|v| v.name.as_str()).collect()
}

/// The loss items of `report` with `reason`.
fn items<'a>(report: &'a LossReport, reason: &str) -> Vec<&'a LossItem> {
    report.items.iter().filter(|i| i.reason == reason).collect()
}

/// The `names` argument of a loss item, as text.
fn listed(item: &LossItem) -> String {
    match item.arguments.get("names") {
        Some(Argument::Text(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Writes a drawing with `header` and one circle, imports it into a new space and returns the
/// environment and the space.
fn import_with(header: Vec<HeaderVariable>, env: &mut Env) -> arc_base::ObjectId {
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], None);
    d.header = header;
    let space = env.space("model", "millimeter");
    let path = env.write("h.dxf", &bytes(&d));
    env.import(&path, space).unwrap();
    space
}

#[test]
fn derived_variables_are_omitted_and_reported_as_one_item() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$DIMTXT", vec![Group::double(40, 2.5)]),
            var(
                "$EXTMIN",
                vec![
                    Group::double(10, -1.0),
                    Group::double(20, -1.0),
                    Group::double(30, 0.0),
                ],
            ),
            var(
                "$EXTMAX",
                vec![
                    Group::double(10, 9.0),
                    Group::double(20, 9.0),
                    Group::double(30, 0.0),
                ],
            ),
            var("$TDUPDATE", vec![Group::double(40, 2_460_000.5)]),
            var("$TDCREATE", vec![Group::double(40, 2_460_000.25)]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    assert_eq!(
        names(&out.drawing.header),
        ["$DIMTXT", "$TDCREATE", "$INSUNITS", "$HANDSEED"]
    );
    let omitted = items(&out.report, "draft.dxf.header_derived_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(omitted[0].arguments.get("count"), Some(&Argument::Int(3)));
    assert_eq!(listed(omitted[0]), "$EXTMIN, $EXTMAX, $TDUPDATE");
}

#[test]
fn a_handle_is_written_only_when_the_export_writes_its_object_under_that_handle() {
    let mut env = Env::new();
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], None);
    // The layer WALLS keeps the source handle 10.
    d.tables.layers.as_mut().unwrap().entries[1].head.handle = Some(Handle(0x10));
    d.header = vec![
        var("$XREF", vec![Group::text(340, "10")]),
        var("$XGONE", vec![Group::text(340, "77")]),
    ];
    let space = env.space("model", "millimeter");
    let path = env.write("h.dxf", &bytes(&d));
    env.import(&path, space).unwrap();

    // The document label of the import is the file name.
    let snapshot = env.snapshot();
    let keep = HandlePolicy::KeepSources {
        document: "h.dxf".to_owned(),
        floor: 0x100,
    };
    let kept = export_drawing_with(&snapshot, space, &keep).unwrap();
    assert!(names(&kept.drawing.header).contains(&"$XREF"));
    assert!(!names(&kept.drawing.header).contains(&"$XGONE"));
    let omitted = items(&kept.report, "draft.dxf.header_handle_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(listed(omitted[0]), "$XGONE");

    // Without the source bindings no object is written under a file handle: both are omitted.
    let plain = export_drawing(&snapshot, space).unwrap();
    assert!(!names(&plain.drawing.header).contains(&"$XREF"));
    let omitted = items(&plain.report, "draft.dxf.header_handle_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(listed(omitted[0]), "$XREF, $XGONE");
}

#[test]
fn a_name_is_written_only_when_the_file_has_the_table_entry() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$CLAYER", vec![Group::text(8, "walls")]),
            var("$CELTYPE", vec![Group::text(6, "NOPE")]),
            var("$DIMBLK", vec![Group::text(1, "")]),
            var("$DIMBLK1", vec![Group::text(1, "NOBLOCK")]),
            var("$CMLSTYLE", vec![Group::text(2, "STANDARD")]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    let kept = names(&out.drawing.header);
    // The layer is matched without regard to case, and an empty block name names no block.
    assert!(kept.contains(&"$CLAYER"));
    assert!(kept.contains(&"$DIMBLK"));
    assert!(!kept.contains(&"$CELTYPE"));
    assert!(!kept.contains(&"$DIMBLK1"));
    // The export writes no multiline style table.
    assert!(!kept.contains(&"$CMLSTYLE"));
    let omitted = items(&out.report, "draft.dxf.header_name_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(listed(omitted[0]), "$CELTYPE, $DIMBLK1, $CMLSTYLE");
}

#[test]
fn values_that_do_not_fit_their_group_code_are_omitted_by_the_import() {
    let mut d = drawing(vec![], None);
    d.header = vec![
        var("$DIMTXT", vec![Group::double(40, 2.5)]),
        var(
            "$BADREAL",
            vec![Group {
                code: 40,
                value: Value::Double(f64::NAN),
            }],
        ),
        var(
            "$BADKIND",
            vec![Group {
                code: 70,
                value: Value::Text("x".to_owned()),
            }],
        ),
        var("$DIMCLRD", vec![Group::int(70, 3)]),
        var("$ZZ_UNKNOWN", vec![Group::text(999, "kept as text")]),
    ];
    let p = plan(&d, &ImportOptions::default());
    let kept: Vec<String> = p
        .foreign
        .iter()
        .filter(|r| r.record_kind == FOREIGN_HEADER)
        .filter_map(|r| decode_record(&r.data).map(|rec| rec.kind))
        .collect();
    assert_eq!(kept, ["$DIMTXT", "$DIMCLRD", "$ZZ_UNKNOWN"]);
    let omitted = items(&p.report, "draft.dxf.header_value_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(listed(omitted[0]), "$BADREAL, $BADKIND");
}

#[test]
fn the_code_page_is_not_written_because_the_output_is_utf8() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$DWGCODEPAGE", vec![Group::text(3, "ANSI_1252")]),
            var("$DIMTXT", vec![Group::double(40, 2.5)]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    assert!(!names(&out.drawing.header).contains(&"$DWGCODEPAGE"));
    let omitted = items(&out.report, "draft.dxf.header_value_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(listed(omitted[0]), "$DWGCODEPAGE");
}
