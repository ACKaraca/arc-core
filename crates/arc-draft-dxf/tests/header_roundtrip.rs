//! Header variables the import does not consume are kept as foreign rows of the target space,
//! written back in their stored order on export, removed by undo and bounded in count and size.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::Limits;
use arc_draft_dxf::plan::{FOREIGN_HEADER, MAX_HEADER_VARIABLES};
use arc_draft_dxf::{ImportOptions, export_drawing, plan};
use arc_io_dxf::loss::Argument;
use arc_io_dxf::model::HeaderVariable;
use arc_io_dxf::{Group, Value};
use support::{Env, bytes, circle, drawing};

fn var(name: &str, values: Vec<Group>) -> HeaderVariable {
    HeaderVariable {
        name: name.to_owned(),
        values,
    }
}

/// Twelve assorted variables: strings, integers, reals, points with several group codes, an
/// unknown name and an angle in degrees. `$INSUNITS` is consumed by the import; the extents are
/// derived and omitted by the export (see `header_classes.rs`).
fn assorted() -> Vec<HeaderVariable> {
    vec![
        var("$DIMTXT", vec![Group::double(40, 2.5)]),
        var("$DIMCLRD", vec![Group::int(70, 3)]),
        var("$INSUNITS", vec![Group::int(70, 4)]),
        var("$CLAYER", vec![Group::text(8, "WALLS")]),
        var(
            "$EXTMIN",
            vec![
                Group::double(10, -1.0),
                Group::double(20, -2.0),
                Group::double(30, 0.0),
            ],
        ),
        var(
            "$EXTMAX",
            vec![
                Group::double(10, 10.0),
                Group::double(20, 20.0),
                Group::double(30, 0.0),
            ],
        ),
        var("$ANGBASE", vec![Group::double(50, 90.0)]),
        var("$TEXTSIZE", vec![Group::double(40, 0.125)]),
        var("$TDCREATE", vec![Group::double(40, 2_460_000.5)]),
        var("$DIMPOST", vec![Group::text(1, "mm")]),
        var(
            "$ZZ_UNKNOWN",
            vec![
                Group::text(1, "kept"),
                Group::int(70, -7),
                Group::double(40, 0.001),
            ],
        ),
        var("$DIMSAH", vec![Group::int(70, 1)]),
    ]
}

/// The bytes of a drawing with the header `header` and one circle.
fn with_header(header: Vec<HeaderVariable>) -> Vec<u8> {
    let mut d = drawing(vec![circle([0.0; 3], 1.0)], None);
    d.header = header;
    bytes(&d)
}

fn names(header: &[HeaderVariable]) -> Vec<&str> {
    header.iter().map(|v| v.name.as_str()).collect()
}

#[test]
fn unconsumed_variables_round_trip_in_their_order() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("h.dxf", &with_header(assorted()));
    let before = env.snapshot().foreign_len();
    let result = env.import(&path, space).unwrap().value;
    assert_eq!(result["entities"], 1);
    assert_eq!(env.snapshot().foreign_of(space).count(), 11);

    let exported = env.exported(space);
    // The stored variables that the export may write come back as they were, in order. $INSUNITS
    // is consumed by the import, so the export owns it and writes it after them, then $HANDSEED.
    let kept: Vec<HeaderVariable> = assorted()
        .into_iter()
        .filter(|v| !["$INSUNITS", "$EXTMIN", "$EXTMAX"].contains(&v.name.as_str()))
        .collect();
    assert_eq!(exported.header.len(), kept.len() + 2);
    assert_eq!(exported.header[..kept.len()], kept[..]);
    assert_eq!(
        names(&exported.header[kept.len()..]),
        ["$INSUNITS", "$HANDSEED"]
    );

    // The written file holds the variables in the same order.
    let text = String::from_utf8_lossy(&bytes(&exported)).into_owned();
    let at: Vec<usize> = ["$DIMTXT", "$DIMCLRD", "$CLAYER", "$ANGBASE", "$ZZ_UNKNOWN"]
        .iter()
        .map(|name| text.find(name).unwrap_or(usize::MAX))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{at:?}");

    // One undo removes the rows.
    assert!(env.document.undo().wait().unwrap().value);
    assert_eq!(env.snapshot().foreign_len(), before);
    assert_eq!(env.snapshot().foreign_of(space).count(), 0);
}

#[test]
fn an_angle_in_degrees_stays_in_degrees_on_the_drafting_path() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("a.dxf", &with_header(assorted()));
    env.import(&path, space).unwrap();
    let angle = env
        .exported(space)
        .header
        .into_iter()
        .find(|v| v.name == "$ANGBASE")
        .map(|v| v.values);
    assert_eq!(angle, Some(vec![Group::double(50, 90.0)]));
}

#[test]
fn an_owned_variable_takes_the_place_of_a_stored_one_of_its_name() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let header = vec![
        var("$DIMTXT", vec![Group::double(40, 2.5)]),
        var("$HANDSEED", vec![Group::text(5, "FF")]),
        var("$DIMCLRD", vec![Group::int(70, 3)]),
    ];
    let path = env.write("s.dxf", &with_header(header));
    env.import(&path, space).unwrap();
    let exported = env.exported(space).header;
    // The file had no $INSUNITS, so the export's own one follows the stored variables.
    assert_eq!(
        names(&exported),
        ["$DIMTXT", "$HANDSEED", "$DIMCLRD", "$INSUNITS"]
    );
    let seed = exported.iter().find(|v| v.name == "$HANDSEED");
    assert_eq!(
        seed.map(|v| v.values.clone()),
        Some(vec![Group::text(5, "1")])
    );
}

#[test]
fn the_count_bound_keeps_the_first_variables_and_reports_the_rest() {
    let header: Vec<HeaderVariable> = (0..MAX_HEADER_VARIABLES + 6)
        .map(|i| {
            var(
                &format!("$V{i}"),
                vec![Group::int(70, i64::try_from(i).unwrap_or(0))],
            )
        })
        .collect();
    let mut d = drawing(vec![], None);
    d.header = header;
    let p = plan(&d, &ImportOptions::default());
    let rows: Vec<_> = p
        .foreign
        .iter()
        .filter(|r| r.record_kind == FOREIGN_HEADER)
        .collect();
    assert_eq!(rows.len(), MAX_HEADER_VARIABLES);
    assert_eq!(overflow(&p.report), 6);
}

#[test]
fn the_string_bound_counts_the_bytes_of_the_kept_variables() {
    // Each variable here is 27 bytes encoded: two fit in 60 bytes, the third does not.
    let header: Vec<HeaderVariable> = (0..3)
        .map(|i| var(&format!("$DIMTXT{i}"), vec![Group::double(40, 2.5)]))
        .collect();
    let mut d = drawing(vec![], None);
    d.header = header;
    let options = ImportOptions {
        limits: Limits {
            string_bytes: 60,
            ..Limits::DEFAULT
        },
        ..ImportOptions::default()
    };
    let p = plan(&d, &options);
    assert_eq!(
        p.foreign
            .iter()
            .filter(|r| r.record_kind == FOREIGN_HEADER)
            .count(),
        2
    );
    assert_eq!(overflow(&p.report), 1);
}

#[test]
fn a_drawing_that_was_not_imported_exports_the_same_header() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let exported = export_drawing(&env.snapshot(), space).unwrap().drawing;
    assert_eq!(
        exported.header,
        vec![
            var("$INSUNITS", vec![Group::int(70, 4)]),
            var("$HANDSEED", vec![Group::text(5, "1")]),
        ]
    );
    assert_eq!(exported.header[0].values[0].value, Value::Int(4));
}

/// The number of variables the report counts as overflowing the header bounds.
fn overflow(report: &arc_io_dxf::LossReport) -> i64 {
    report
        .items
        .iter()
        .filter(|i| {
            i.reason == "draft.dxf.foreign_overflow" && i.record_kind.as_deref() == Some("HEADER")
        })
        .map(|i| match i.arguments.get("count") {
            Some(Argument::Int(n)) => *n,
            _ => 1,
        })
        .sum()
}
