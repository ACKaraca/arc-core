//! Review of the header rows: a variable must be complete and have a valid name; a second import
//! replaces the rows of its space; lengths follow the unit conversion; the derived, handle and
//! name classes cover the variables the export would write stale or dangling; and the export
//! omits every stored value the writer refuses.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier as _;
use arc_base::{Limits, ObjectId};
use arc_doc::{Snapshot, StoredForeign};
use arc_draft_dxf::plan::{
    FOREIGN_FORMAT, FOREIGN_HEADER, FOREIGN_OBJECT, decode_record, encode_record,
};
use arc_draft_dxf::{HandlePolicy, ImportOptions, export_drawing, export_drawing_for_mode, plan};
use arc_io_dxf::loss::{Argument, LossItem, LossReport};
use arc_io_dxf::model::{HeaderVariable, PreservedRecord};
use arc_io_dxf::{Group, Version, WriteOptions, write};
use support::{Env, bytes, circle, drawing};

fn var(name: &str, values: Vec<Group>) -> HeaderVariable {
    HeaderVariable {
        name: name.to_owned(),
        values,
    }
}

fn items<'a>(report: &'a LossReport, reason: &str) -> Vec<&'a LossItem> {
    report.items.iter().filter(|i| i.reason == reason).collect()
}

fn listed(item: &LossItem) -> String {
    match item.arguments.get("names") {
        Some(Argument::Text(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The header variables the import keeps, as (name, groups), in order.
fn kept(p: &arc_draft_dxf::Plan) -> Vec<(String, Vec<Group>)> {
    p.foreign
        .iter()
        .filter(|r| r.record_kind == FOREIGN_HEADER)
        .filter_map(|r| decode_record(&r.data).map(|rec| (rec.kind, rec.groups)))
        .collect()
}

fn kept_names(p: &arc_draft_dxf::Plan) -> Vec<String> {
    kept(p).into_iter().map(|(name, _)| name).collect()
}

fn names(header: &[HeaderVariable]) -> Vec<&str> {
    header.iter().map(|v| v.name.as_str()).collect()
}

/// Imports a drawing with `header` and one circle into a new millimeter space.
fn import_with(header: Vec<HeaderVariable>, env: &mut Env) -> ObjectId {
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], None);
    d.header.extend(header);
    let space = env.space("model", "millimeter");
    let path = env.write("h.dxf", &bytes(&d));
    env.import(&path, space).unwrap();
    space
}

/// Adds a stored header row to `space` holding `groups` under `name`, as another writer would.
fn store_header(env: &Env, space: ObjectId, name: &str, groups: Vec<Group>) {
    let data = encode_record(&PreservedRecord {
        kind: name.to_owned(),
        groups,
    });
    let row = StoredForeign {
        id: 0,
        source_format: FOREIGN_FORMAT.to_owned(),
        source_version: Some("2018".to_owned()),
        source_id: None,
        object: Some(space.as_u128().to_be_bytes().to_vec()),
        record_kind: FOREIGN_HEADER.to_owned(),
        data,
        asset: None,
    };
    env.document
        .submit(move |txn| txn.add_foreign(row).map(drop))
        .wait()
        .unwrap();
}

fn header_rows(snapshot: &Snapshot, space: ObjectId) -> usize {
    snapshot
        .foreign_of(space)
        .filter(|r| r.source_format == FOREIGN_FORMAT && r.record_kind == FOREIGN_HEADER)
        .count()
}

#[test]
fn a_variable_must_have_a_complete_value_and_a_valid_name() {
    let mut d = drawing(vec![], None);
    d.header = vec![
        var(
            "$LIMMAX",
            vec![Group::double(10, 10.0), Group::double(20, 20.0)],
        ),
        // An X without its Y.
        var("$LIMMIN", vec![Group::double(10, 0.0)]),
        // A Y without its X.
        var("$VIEWCTR", vec![Group::double(20, 5.0)]),
        // A Z that does not follow its Y.
        var(
            "$ZBAD",
            vec![
                Group::double(10, 1.0),
                Group::double(30, 1.0),
                Group::double(20, 1.0),
            ],
        ),
        var("$NOVALUES", vec![]),
        var("$BAD NAME", vec![Group::double(40, 1.0)]),
        var("EXTMIN2", vec![Group::double(40, 1.0)]),
    ];
    let p = plan(&d, &ImportOptions::default());
    assert_eq!(kept_names(&p), ["$LIMMAX"]);
    let omitted = items(&p.report, "draft.dxf.header_value_omitted");
    assert_eq!(omitted.len(), 1);
    assert_eq!(
        listed(omitted[0]),
        "$LIMMIN, $VIEWCTR, $ZBAD, $NOVALUES, $BAD NAME, EXTMIN2"
    );
}

#[test]
fn names_are_compared_without_regard_to_case_and_written_as_the_file_has_them() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$dimtxt", vec![Group::double(40, 2.5)]),
            var("$DIMTXT", vec![Group::double(40, 3.0)]),
            var(
                "$extmin",
                vec![
                    Group::double(10, 0.0),
                    Group::double(20, 0.0),
                    Group::double(30, 0.0),
                ],
            ),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    // One row for the name, under the spelling the file had first and with the last groups.
    let text: Vec<&HeaderVariable> = out
        .drawing
        .header
        .iter()
        .filter(|v| v.name.eq_ignore_ascii_case("$dimtxt"))
        .collect();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].name, "$dimtxt");
    assert_eq!(text[0].values, vec![Group::double(40, 3.0)]);
    // The derived list is matched without regard to case.
    let derived = items(&out.report, "draft.dxf.header_derived_omitted");
    assert_eq!(listed(derived[0]), "$extmin");
}

#[test]
fn lengths_follow_the_unit_conversion_and_the_measurement_variables_follow_the_target() {
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], Some(1));
    d.header.extend([
        var("$TEXTSIZE", vec![Group::double(40, 0.5)]),
        var(
            "$LIMMAX",
            vec![Group::double(10, 10.0), Group::double(20, 20.0)],
        ),
        var("$PDSIZE", vec![Group::double(40, 2.0)]),
        var("$LTSCALE", vec![Group::double(40, 2.0)]),
        var("$CHAMFERD", vec![Group::double(50, 90.0)]),
        var("$MEASUREMENT", vec![Group::int(70, 0)]),
        var("$LUNITS", vec![Group::int(70, 4)]),
        var("$ZZ_UNKNOWN", vec![Group::double(40, 2.0)]),
    ]);
    // The file is in inches; the model space is in millimeters.
    let p = plan(&d, &ImportOptions::default());
    let rows = kept(&p);
    let get = |name: &str| rows.iter().find(|(n, _)| n == name).map(|(_, g)| g.clone());
    let scaled = |v: f64| Some(vec![Group::double(40, v)]);
    assert_eq!(get("$TEXTSIZE"), scaled(12.7));
    assert_eq!(
        get("$LIMMAX"),
        Some(vec![Group::double(10, 254.0), Group::double(20, 508.0)])
    );
    assert_eq!(get("$PDSIZE"), scaled(50.8));
    // A ratio and an angle keep their values.
    assert_eq!(get("$LTSCALE"), scaled(2.0));
    assert_eq!(get("$CHAMFERD"), Some(vec![Group::double(50, 90.0)]));
    // The measurement follows the target (metric); the linear format of the file (imperial) is
    // no longer its format and is omitted.
    assert_eq!(get("$MEASUREMENT"), Some(vec![Group::int(70, 1)]));
    assert_eq!(get("$LUNITS"), None);
    let derived = items(&p.report, "draft.dxf.header_derived_omitted");
    assert_eq!(listed(derived[0]), "$LUNITS");
    // The unknown length is kept as it is, with one item for all unknown variables.
    assert_eq!(get("$ZZ_UNKNOWN"), scaled(2.0));
    let unscaled = items(&p.report, "draft.dxf.header_unscaled");
    assert_eq!(unscaled.len(), 1);
    assert_eq!(listed(unscaled[0]), "$ZZ_UNKNOWN");
}

#[test]
fn the_update_time_the_user_timer_and_the_paper_extents_are_derived() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$TDUUPDATE", vec![Group::double(40, 2_460_000.5)]),
            var("$TDUSRTIMER", vec![Group::double(40, 1.0)]),
            var(
                "$PEXTMIN",
                vec![Group::double(10, 0.0), Group::double(20, 0.0)],
            ),
            var(
                "$PEXTMAX",
                vec![Group::double(10, 5.0), Group::double(20, 5.0)],
            ),
            var("$TDCREATE", vec![Group::double(40, 2_460_000.25)]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    assert_eq!(
        names(&out.drawing.header),
        ["$TDCREATE", "$INSUNITS", "$HANDSEED"]
    );
    let derived = items(&out.report, "draft.dxf.header_derived_omitted");
    assert_eq!(
        listed(derived[0]),
        "$TDUUPDATE, $TDUSRTIMER, $PEXTMIN, $PEXTMAX"
    );
}

#[test]
fn handle_groups_390_to_399_and_480_481_need_the_object_they_name() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$HDL390", vec![Group::text(390, "10")]),
            var("$HDL480", vec![Group::text(480, "10")]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    assert!(!names(&out.drawing.header).contains(&"$HDL390"));
    assert!(!names(&out.drawing.header).contains(&"$HDL480"));
    let omitted = items(&out.report, "draft.dxf.header_handle_omitted");
    assert_eq!(listed(omitted[0]), "$HDL390, $HDL480");
}

#[test]
fn text_style_linetype_and_ucs_names_are_checked_against_the_tables() {
    let mut env = Env::new();
    let space = import_with(
        vec![
            var("$DIMTXSTY", vec![Group::text(7, "NOSTYLE")]),
            var("$DIMLTYPE", vec![Group::text(6, "NOPE")]),
            // No UCS table is written: a named UCS is omitted; the empty names are the world.
            var("$UCSNAME", vec![Group::text(2, "MYUCS")]),
            var("$UCSBASE", vec![Group::text(2, "")]),
            var("$PUCSNAME", vec![Group::text(2, "")]),
            var("$PUCSBASE", vec![Group::text(2, "MYUCS")]),
        ],
        &mut env,
    );
    let out = export_drawing(&env.snapshot(), space).unwrap();
    let kept = names(&out.drawing.header);
    assert!(!kept.contains(&"$DIMTXSTY"));
    assert!(!kept.contains(&"$DIMLTYPE"));
    assert!(!kept.contains(&"$UCSNAME"));
    assert!(kept.contains(&"$UCSBASE"));
    assert!(kept.contains(&"$PUCSNAME"));
    assert!(!kept.contains(&"$PUCSBASE"));
    let omitted = items(&out.report, "draft.dxf.header_name_omitted");
    assert_eq!(
        listed(omitted[0]),
        "$DIMTXSTY, $DIMLTYPE, $UCSNAME, $PUCSBASE"
    );
}

#[test]
fn a_second_import_replaces_the_header_rows_of_its_space() {
    let mut env = Env::new();
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], None);
    d.header.push(var("$DIMTXT", vec![Group::double(40, 2.5)]));
    let space = env.space("model", "millimeter");
    let path = env.write("h.dxf", &bytes(&d));
    env.import(&path, space).unwrap();
    let once = header_rows(&env.snapshot(), space);
    assert_eq!(once, 1);
    env.import(&path, space).unwrap();
    assert_eq!(header_rows(&env.snapshot(), space), once);
}

#[test]
fn a_stored_value_the_writer_refuses_is_omitted_and_the_write_succeeds() {
    let mut env = Env::new();
    let space = import_with(vec![var("$DIMTXT", vec![Group::double(40, 2.5)])], &mut env);
    // An integer beyond its code's range: no file holds it, another writer may have stored it.
    store_header(&env, space, "$TOOBIG", vec![Group::int(70, 70_000)]);
    // A text in a code the crate does not know: the ASCII writer takes it, the binary one does not.
    store_header(&env, space, "$UNKNOWNCODE", vec![Group::text(150, "x")]);

    let snapshot = env.snapshot();
    let ascii = export_drawing_for_mode(
        &snapshot,
        space,
        &HandlePolicy::Assign,
        &Limits::DEFAULT,
        false,
    )
    .unwrap();
    assert!(
        write(
            &ascii.drawing,
            &mut WriteOptions::for_version(Version::R2018)
        )
        .is_ok()
    );
    assert!(names(&ascii.drawing.header).contains(&"$UNKNOWNCODE"));
    assert!(!names(&ascii.drawing.header).contains(&"$TOOBIG"));

    let binary = export_drawing_for_mode(
        &snapshot,
        space,
        &HandlePolicy::Assign,
        &Limits::DEFAULT,
        true,
    )
    .unwrap();
    let mut options = WriteOptions::for_version(Version::R2018);
    options.binary = true;
    assert!(write(&binary.drawing, &mut options).is_ok());
    assert!(!names(&binary.drawing.header).contains(&"$UNKNOWNCODE"));
    let omitted = items(&binary.report, "draft.dxf.header_value_omitted");
    assert_eq!(listed(omitted[0]), "$TOOBIG, $UNKNOWNCODE");
}

#[test]
fn a_stored_record_the_writer_refuses_does_not_fail_the_export() {
    let mut env = Env::new();
    let space = import_with(vec![], &mut env);
    let record = encode_record(&PreservedRecord {
        kind: "ACME_OBJ".to_owned(),
        groups: vec![Group::int(70, 70_000)],
    });
    let row = StoredForeign {
        id: 0,
        source_format: FOREIGN_FORMAT.to_owned(),
        source_version: Some("2018".to_owned()),
        source_id: None,
        object: Some(space.as_u128().to_be_bytes().to_vec()),
        record_kind: FOREIGN_OBJECT.to_owned(),
        data: record,
        asset: None,
    };
    env.document
        .submit(move |txn| txn.add_foreign(row).map(drop))
        .wait()
        .unwrap();
    let out = export_drawing(&env.snapshot(), space).unwrap();
    assert!(write(&out.drawing, &mut WriteOptions::for_version(Version::R2018)).is_ok());
    assert_eq!(items(&out.report, "draft.dxf.export_unmapped").len(), 1);
}
