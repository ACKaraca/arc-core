//! Tables (CORE-DRAFT-008): merged-range validation, formulas against a reference evaluator,
//! cycles, row types and the table style, column breaking with repeated headers, grid lines,
//! edits, fields in cells and the package round trip.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use std::collections::BTreeMap;

use arc_base::math::Point2;
use arc_base::{ObjectId, Tolerance};
use arc_draft::display::{self, DisplayOptions, Shape};
use arc_draft::table::{self, Placed, Table};
use arc_geom::{Curve2, Segment2};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{Fixture, curve_json, round_trip};

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn rows(kinds: &[(&str, f64)]) -> Value {
    Value::Array(
        kinds
            .iter()
            .map(|(k, h)| json!({"kind": k, "height": h}))
            .collect(),
    )
}

fn data_rows(n: usize, height: f64) -> Value {
    rows(&vec![("data", height); n])
}

fn make(f: &mut Fixture, owner: ObjectId, columns: &[f64], rows: Value, cells: Value) -> ObjectId {
    f.create(
        "core.draft.table.create",
        json!({"owner": s(owner), "columns": columns, "rows": rows, "cells": cells}),
    )
}

fn number(cell: &str, value: f64) -> Value {
    json!({"cell": cell, "content": {"kind": "number", "value": value}})
}

fn formula(cell: &str, expression: &str) -> Value {
    json!({"cell": cell, "content": {"kind": "formula", "expression": expression}})
}

fn word(cell: &str, text: &str) -> Value {
    json!({"cell": cell, "content": {"kind": "text", "text": text}})
}

fn resolution(f: &Fixture, id: ObjectId) -> table::Resolution {
    table::resolve(&f.document.snapshot(), id, &DisplayOptions::default()).unwrap()
}

fn shown(f: &Fixture, id: ObjectId, cell: &str) -> String {
    let (r, c) = table::parse_cell(cell).unwrap();
    resolution(f, id)
        .texts
        .get(&(r, c))
        .cloned()
        .unwrap_or_default()
}

fn warnings(f: &Fixture) -> Vec<String> {
    f.document
        .diagnostics_snapshot()
        .diagnostics
        .iter()
        .map(|d| d.code().to_owned())
        .collect()
}

fn list(f: &Fixture, id: ObjectId) -> Vec<arc_draft::display::Primitive> {
    display::entity_list(&f.document.snapshot(), id, &DisplayOptions::default())
        .unwrap()
        .primitives
}

fn segments(f: &Fixture, id: ObjectId) -> Vec<Segment2> {
    list(f, id)
        .into_iter()
        .filter(|p| p.part == 0)
        .map(|p| match p.shape {
            Shape::Stroke {
                curve: Curve2::Segment(s),
                ..
            } => s,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn cell_names_round_trip() {
    for (row, column, name) in [(0, 0, "A1"), (4, 25, "Z5"), (0, 26, "AA1"), (9, 27, "AB10")] {
        assert_eq!(table::cell_name(row, column), name);
        assert_eq!(table::parse_cell(name), Some((row, column)));
    }
    for bad in ["", "a1", "A0", "A01", "1A", "AAAA1", "A", "A1B"] {
        assert_eq!(table::parse_cell(bad), None, "{bad}");
    }
}

#[test]
fn merged_ranges_are_validated_and_drop_covered_content() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let t = make(
        &mut f,
        space,
        &[10.0, 10.0, 10.0],
        data_rows(3, 5.0),
        json!([word("A1", "a"), word("B2", "covered"), number("C3", 1.0)]),
    );
    let merge = |f: &mut Fixture, range: &str| {
        f.run(
            "core.draft.table.merge",
            json!({"table": s(t), "range": range}),
        )
    };
    merge(&mut f, "A1:B2").unwrap();
    let stored = Table::from_values(&f.values(t));
    assert_eq!(stored.merges.len(), 1);
    // Only the first cell of a merged range keeps its content.
    assert!(stored.cell(1, 1).is_none());
    assert!(stored.cell(0, 0).is_some());
    for (range, why) in [
        ("B2:C3", "overlaps"),
        ("A1:A1", "one cell"),
        ("A1:D3", "outside the grid"),
        ("C3:C3", "one cell"),
        ("A9:B9", "outside the grid"),
    ] {
        let error = merge(&mut f, range).unwrap_err();
        assert_eq!(error.code(), "draft.table.merge_invalid", "{why}");
    }
    // Corners in any order give the same rectangle.
    merge(&mut f, "C3:B3").unwrap();
    assert_eq!(Table::from_values(&f.values(t)).merges.len(), 2);
    // Content in a covered cell is refused.
    let error = f
        .run(
            "core.draft.table.cell.set",
            json!({"table": s(t), "cell": "B1", "content": {"kind": "number", "value": 2.0}}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.table.cell_invalid");
    // A cell name that is not a name, and one outside the grid.
    for cell in ["b1", "D1", "A4"] {
        let error = f
            .run(
                "core.draft.table.cell.set",
                json!({"table": s(t), "cell": cell,
                       "content": {"kind": "number", "value": 2.0}}),
            )
            .unwrap_err();
        assert_eq!(error.code(), "draft.table.cell_invalid", "{cell}");
    }
    // Unmerging removes the range; a cell outside every range is refused.
    f.run(
        "core.draft.table.unmerge",
        json!({"table": s(t), "range": "B2"}),
    )
    .unwrap();
    assert_eq!(Table::from_values(&f.values(t)).merges.len(), 1);
    let error = f
        .run(
            "core.draft.table.unmerge",
            json!({"table": s(t), "range": "A3"}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.table.merge_invalid");
    // A formula that does not parse is refused.
    let error = f
        .run(
            "core.draft.table.cell.set",
            json!({"table": s(t), "cell": "A2", "content": {"kind": "formula", "expression": "1 +"}}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.table.formula_invalid");
}

#[test]
fn grids_are_validated() {
    let mut f = Fixture::new();
    let space = f.space("model");
    // The parameter schema already refuses an empty grid; the validator catches the rest.
    for (columns, rows_json, code) in [
        (json!([]), data_rows(1, 1.0), "cmd.params.invalid"),
        (json!([1.0]), json!([]), "cmd.params.invalid"),
        (json!([0.0]), data_rows(1, 1.0), "draft.table.grid_invalid"),
        (json!([1.0]), data_rows(1, -2.0), "draft.table.grid_invalid"),
    ] {
        let error = f
            .run(
                "core.draft.table.create",
                json!({"owner": s(space), "columns": columns, "rows": rows_json}),
            )
            .unwrap_err();
        assert_eq!(error.code(), code, "{columns}");
    }
    let mut big = Table::from_values(&[]);
    big.rows = vec![table::Row {
        kind: table::RowKind::Data,
        height: 1.0,
    }];
    big.widths = vec![1.0; table::MAX_COLUMNS + 1];
    assert_eq!(
        big.validate(&arc_base::Limits::DEFAULT).unwrap_err().code(),
        "draft.table.too_large"
    );
    big.widths = vec![1.0];
    big.rows = vec![big.rows[0]; table::MAX_ROWS + 1];
    assert_eq!(
        big.validate(&arc_base::Limits::DEFAULT).unwrap_err().code(),
        "draft.table.too_large"
    );
    // The last row and the last column cannot be deleted.
    let t = make(&mut f, space, &[1.0], data_rows(1, 1.0), json!([]));
    for command in [
        "core.draft.table.row.delete",
        "core.draft.table.column.delete",
    ] {
        let error = f.run(command, json!({"table": s(t), "at": 0})).unwrap_err();
        assert_eq!(error.code(), "draft.table.grid_invalid", "{command}");
    }
}

/// One generated cell of the reference model.
#[derive(Debug, Clone)]
struct Gen {
    kind: u8,
    a: usize,
    b: usize,
    c: usize,
    n: i32,
}

fn gen_strategy() -> impl Strategy<Value = Gen> {
    (0_u8..6, 0_usize..64, 0_usize..64, 0_usize..64, 0_i32..10).prop_map(|(kind, a, b, c, n)| Gen {
        kind,
        a,
        b,
        c,
        n,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Random acyclic tables give the values of an independent evaluator that follows the
    /// reading order; a formula only reads cells before it.
    #[test]
    fn formulas_match_a_reference_evaluator(
        rows_n in 1_usize..5,
        columns_n in 1_usize..5,
        gens in proptest::collection::vec(gen_strategy(), 16),
    ) {
        let mut f = Fixture::new();
        let space = f.space("model");
        let mut cells = Vec::new();
        let mut expected: BTreeMap<String, Option<f64>> = BTreeMap::new();
        let mut values: Vec<f64> = Vec::new();
        for i in 0..rows_n * columns_n {
            let g = &gens[i];
            let name = table::cell_name(i / columns_n, i % columns_n);
            let name_of = |k: usize| table::cell_name((k % i.max(1)) / columns_n, (k % i.max(1)) % columns_n);
            let at = |k: usize| values[k % i.max(1)];
            // A cell that reads earlier cells needs an earlier cell to read.
            let kind = if i == 0 && g.kind >= 2 { 0 } else { g.kind };
            let (value, entry) = match kind {
                0 | 1 => (f64::from(g.n), Some(number(&name, f64::from(g.n)))),
                2 => (at(g.a) + at(g.b), Some(formula(&name, &format!("{} + {}", name_of(g.a), name_of(g.b))))),
                3 => {
                    let k = 2 + g.n % 2;
                    (at(g.a) * f64::from(k), Some(formula(&name, &format!("{} * {k}", name_of(g.a)))))
                }
                4 => (
                    at(g.a) + at(g.b) + at(g.c),
                    Some(formula(&name, &format!("sum([{}, {}, {}])", name_of(g.a), name_of(g.b), name_of(g.c)))),
                ),
                _ => (f64::from(g.n) - at(g.a), Some(formula(&name, &format!("{} - {}", g.n, name_of(g.a))))),
            };
            values.push(value);
            expected.insert(name, Some(value));
            cells.extend(entry);
        }
        let columns = vec![10.0; columns_n];
        let t = make(&mut f, space, &columns, data_rows(rows_n, 5.0), Value::Array(cells));
        let got = resolution(&f, t);
        prop_assert!(got.issues.is_empty(), "{:?}", got.issues);
        for (name, value) in expected {
            let (r, c) = table::parse_cell(&name).unwrap();
            let text = got.texts.get(&(r, c)).cloned().unwrap();
            prop_assert_eq!(text, format!("{}", value.unwrap()), "{}", name);
        }
    }
}

#[test]
fn cycles_give_error_markers_and_a_diagnostic_and_never_hang() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let t = make(
        &mut f,
        space,
        &[10.0; 4],
        data_rows(2, 5.0),
        json!([
            formula("A1", "B1 + 1"),
            formula("B1", "A1 + 1"),
            formula("C1", "A1 * 2"),
            number("D1", 7.0),
            formula("A2", "A2"),
            formula("B2", "D1 + 1"),
            formula("C2", "1 / 0"),
            formula("D2", "nope + 1"),
        ]),
    );
    let r = resolution(&f, t);
    for cell in ["A1", "B1", "C1", "A2"] {
        assert_eq!(shown(&f, t, cell), table::ERROR_MARKER, "{cell}");
    }
    assert_eq!(shown(&f, t, "D1"), "7");
    assert_eq!(shown(&f, t, "B2"), "8");
    // A failing evaluation and an unknown name show the marker too.
    for cell in ["C2", "D2"] {
        assert_eq!(shown(&f, t, cell), table::ERROR_MARKER, "{cell}");
    }
    let cycle = r
        .issues
        .iter()
        .find_map(|i| match i {
            table::Issue::Cycle { cells } => Some(cells.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(cycle, ["A1", "B1", "C1", "A2"]);
    let codes = warnings(&f);
    assert_eq!(
        codes
            .iter()
            .filter(|c| *c == "draft.table.formula_cycle")
            .count(),
        1
    );
    assert_eq!(
        codes
            .iter()
            .filter(|c| *c == "draft.table.formula_invalid")
            .count(),
        2
    );
    // Breaking the cycle clears the markers and the cycle warning.
    f.run(
        "core.draft.table.cell.set",
        json!({"table": s(t), "cell": "B1", "content": {"kind": "number", "value": 1.0}}),
    )
    .unwrap();
    f.run(
        "core.draft.table.cell.set",
        json!({"table": s(t), "cell": "A2", "content": {"kind": "number", "value": 4.0}}),
    )
    .unwrap();
    assert_eq!(shown(&f, t, "A1"), "2");
    assert_eq!(shown(&f, t, "C1"), "4");
    assert!(!warnings(&f).contains(&"draft.table.formula_cycle".to_owned()));
}

#[test]
fn a_very_long_chain_of_formulas_evaluates_without_recursion() {
    let mut f = Fixture::new();
    let space = f.space("model");
    // 40 rows by 24 columns would be too many cells for one command's JSON; one column of 400.
    let mut cells = vec![number("A1", 1.0)];
    for r in 1..400 {
        cells.push(formula(&format!("A{}", r + 1), &format!("A{r} + 1")));
    }
    let t = make(
        &mut f,
        space,
        &[10.0],
        data_rows(400, 1.0),
        Value::Array(cells),
    );
    assert_eq!(shown(&f, t, "A400"), "400");
    // The same chain, closed into a ring, is one cycle.
    f.run(
        "core.draft.table.cell.set",
        json!({"table": s(t), "cell": "A1", "content": {"kind": "formula", "expression": "A400"}}),
    )
    .unwrap();
    assert_eq!(shown(&f, t, "A200"), table::ERROR_MARKER);
}

#[test]
fn columns_break_after_the_maximum_height_with_repeated_headers() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let mut kinds = vec![("title", 6.0), ("header", 4.0)];
    kinds.extend(vec![("data", 5.0); 10]);
    let t = f.create(
        "core.draft.table.create",
        json!({"owner": s(space), "columns": [10.0, 20.0], "rows": rows(&kinds),
               "max_height": 30.0, "break_gap": 7.0,
               "cells": [word("A1", "T"), word("A2", "H")]}),
    );
    let table = Table::from_values(&f.values(t));
    let placed = table::place_rows(&table, &Tolerance::DEFAULT);
    let at = |segment: usize| -> Vec<&Placed> {
        placed.iter().filter(|p| p.segment == segment).collect()
    };
    // Block 0: the title, the header and four data rows (10 + 4 * 5 = 30).
    let first: Vec<usize> = at(0).iter().map(|p| p.row).collect();
    assert_eq!(first, [0, 1, 2, 3, 4, 5]);
    let tops: Vec<f64> = at(0).iter().map(|p| p.top).collect();
    assert_eq!(tops, [0.0, -6.0, -10.0, -15.0, -20.0, -25.0]);
    assert!(at(0).iter().all(|p| p.x == 0.0 && !p.repeated));
    // Block 1 starts at x = 30 + 7 with the header again, then five data rows (4 + 5 * 5 = 29).
    let second = at(1);
    let rows_1: Vec<usize> = second.iter().map(|p| p.row).collect();
    assert_eq!(rows_1, [1, 6, 7, 8, 9, 10]);
    assert_eq!(
        second.iter().map(|p| p.repeated).collect::<Vec<_>>(),
        [true, false, false, false, false, false]
    );
    assert_eq!(
        second.iter().map(|p| p.top).collect::<Vec<_>>(),
        [0.0, -4.0, -9.0, -14.0, -19.0, -24.0]
    );
    assert!(second.iter().all(|p| p.x == 37.0));
    // Block 2: the header and the last data row; the title is not repeated.
    let third: Vec<usize> = at(2).iter().map(|p| p.row).collect();
    assert_eq!(third, [1, 11]);
    assert!(at(2).iter().all(|p| p.x == 74.0));
    assert_eq!(placed.iter().map(|p| p.segment).max(), Some(2));
    // The header text is drawn once in every block, at the left of its column.
    let mut per_block = [0_usize; 3];
    for p in list(&f, t).iter().filter(|p| p.part == 1) {
        let Shape::Stroke { curve, .. } = &p.shape else {
            panic!("{p:?}")
        };
        let bb = curve.bounding_box().unwrap();
        // The title "T" is in block 0 only; the header "H" is the lower text of each block.
        let block = if bb.min().x < 37.0 {
            0
        } else if bb.min().x < 74.0 {
            1
        } else {
            2
        };
        per_block[block] += 1;
    }
    assert!(per_block[1] > 0 && per_block[2] > 0);
    assert_eq!(
        per_block[1], per_block[2],
        "the same header text in each block"
    );
    // Without a maximum height there is one block; rows of a vertical merge stay together.
    f.set("core.draft.table.modify", &[t], "max_height", json!(0.0));
    let table = Table::from_values(&f.values(t));
    assert!(
        table::place_rows(&table, &Tolerance::DEFAULT)
            .iter()
            .all(|p| p.segment == 0)
    );
    f.set("core.draft.table.modify", &[t], "max_height", json!(30.0));
    f.run(
        "core.draft.table.merge",
        json!({"table": s(t), "range": "A6:A8"}),
    )
    .unwrap();
    let table = Table::from_values(&f.values(t));
    let placed = table::place_rows(&table, &Tolerance::DEFAULT);
    let seg_of = |row: usize| {
        placed
            .iter()
            .find(|p| p.row == row && !p.repeated)
            .unwrap()
            .segment
    };
    assert_eq!(seg_of(5), seg_of(6));
    assert_eq!(seg_of(6), seg_of(7));
    // Row 5 (the fourth data row) now moves with rows 6 and 7 to the next block.
    assert_eq!(seg_of(5), 1);
}

#[test]
fn grid_lines_are_omitted_inside_merged_ranges() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let t = make(
        &mut f,
        space,
        &[10.0, 10.0, 10.0],
        data_rows(3, 5.0),
        json!([]),
    );
    let full = segments(&f, t);
    // 4 horizontal and 4 vertical lines of 3 pieces each.
    assert_eq!(full.len(), 24);
    f.run(
        "core.draft.table.merge",
        json!({"table": s(t), "range": "A1:B2"}),
    )
    .unwrap();
    let merged = segments(&f, t);
    assert_eq!(merged.len(), 20);
    let mids: Vec<(f64, f64)> = merged
        .iter()
        .map(|s| {
            (
                (s.start().x + s.end().x) / 2.0,
                (s.start().y + s.end().y) / 2.0,
            )
        })
        .collect();
    let has = |x: f64, y: f64| {
        mids.iter()
            .any(|m| (m.0 - x).abs() < 1e-9 && (m.1 - y).abs() < 1e-9)
    };
    // The line between rows 0 and 1 over columns 0 and 1, and the line between columns 0 and
    // 1 along rows 0 and 1, are gone; the outline of the merged cell and the rest stay.
    assert!(!has(5.0, -5.0) && !has(15.0, -5.0));
    assert!(!has(10.0, -2.5) && !has(10.0, -7.5));
    assert!(has(25.0, -5.0), "the line right of the merge stays");
    assert!(has(5.0, 0.0) && has(5.0, -10.0) && has(0.0, -2.5) && has(20.0, -7.5));
    assert!(
        has(10.0, -12.5),
        "the line between columns below the merge stays"
    );
}

#[test]
fn the_table_style_maps_row_types_to_alignment_borders_and_number_formats() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "table", "name": "schedule"}),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "cell_margin",
        json!(2.0),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "data",
        json!([null, 2.5, "right", true, "1"]),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "header",
        json!([null, 2.5, "left", false, ""]),
    );
    let t = f.create(
        "core.draft.table.create",
        json!({"owner": s(space), "columns": [30.0], "style": s(style),
               "rows": rows(&[("header", 6.0), ("data", 6.0)]),
               "cells": [word("A1", "h"), number("A2", 2.96)]}),
    );
    assert_eq!(shown(&f, t, "A2"), "3.0");
    // Header borders are off, data borders on: the line between the rows is drawn (the data
    // row asks for it), the line above the header and the verticals of the header are not.
    let lines = segments(&f, t);
    assert_eq!(lines.len(), 1 + 2 + 1);
    // The data text is right aligned: its strokes end within the margin of the right edge.
    let mut right = f64::MIN;
    let mut left_header = f64::MAX;
    for p in list(&f, t).iter().filter(|p| p.part == 1) {
        let Shape::Stroke { curve, .. } = &p.shape else {
            panic!("{p:?}")
        };
        let bb = curve.bounding_box().unwrap();
        if bb.max().y < -6.0 {
            right = right.max(bb.max().x);
        } else {
            left_header = left_header.min(bb.min().x);
        }
    }
    assert!(right <= 28.0 + 1e-6 && right > 20.0, "{right}");
    assert!((2.0 - 1e-6..6.0).contains(&left_header), "{left_header}");
}

#[test]
fn rows_and_columns_insert_and_delete_with_their_cells_and_merges() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let t = make(
        &mut f,
        space,
        &[10.0, 10.0, 10.0],
        data_rows(3, 5.0),
        json!([number("A1", 1.0), number("B2", 2.0), number("C3", 3.0)]),
    );
    f.run(
        "core.draft.table.merge",
        json!({"table": s(t), "range": "B2:C3"}),
    )
    .unwrap();
    let run = |f: &mut Fixture, command: &str, params: Value| f.run(command, params).unwrap();
    // A row inserted inside the merged range grows it; one before it moves it.
    run(
        &mut f,
        "core.draft.table.row.insert",
        json!({"table": s(t), "at": 2, "height": 7.0}),
    );
    let stored = Table::from_values(&f.values(t));
    assert_eq!(stored.rows.len(), 4);
    assert_eq!(stored.rows[2].height, 7.0);
    assert_eq!((stored.merges[0].row, stored.merges[0].rows), (1, 3));
    run(
        &mut f,
        "core.draft.table.row.insert",
        json!({"table": s(t), "at": 0, "kind": "header", "height": 3.0}),
    );
    let stored = Table::from_values(&f.values(t));
    assert_eq!((stored.merges[0].row, stored.merges[0].rows), (2, 3));
    assert!(stored.cell(1, 0).is_some(), "A1 moved down");
    assert!(
        stored.cell(3, 1).is_none() && stored.cell(2, 1).is_some(),
        "B2 moved to B3"
    );
    // Deleting rows and columns removes cells and shrinks the merged range until it is gone.
    run(
        &mut f,
        "core.draft.table.row.delete",
        json!({"table": s(t), "at": 0}),
    );
    run(
        &mut f,
        "core.draft.table.column.insert",
        json!({"table": s(t), "at": 1, "width": 4.0}),
    );
    let stored = Table::from_values(&f.values(t));
    assert_eq!(stored.widths, [10.0, 4.0, 10.0, 10.0]);
    assert_eq!((stored.merges[0].column, stored.merges[0].columns), (2, 2));
    run(
        &mut f,
        "core.draft.table.column.delete",
        json!({"table": s(t), "at": 3}),
    );
    run(
        &mut f,
        "core.draft.table.row.delete",
        json!({"table": s(t), "at": 3}),
    );
    run(
        &mut f,
        "core.draft.table.row.delete",
        json!({"table": s(t), "at": 2}),
    );
    let stored = Table::from_values(&f.values(t));
    assert!(stored.merges.is_empty(), "{:?}", stored.merges);
    assert_eq!((stored.rows.len(), stored.widths.len()), (2, 3));
    assert!(
        f.run(
            "core.draft.table.row.delete",
            json!({"table": s(t), "at": 9})
        )
        .is_err()
    );
    assert!(
        f.run(
            "core.draft.table.row.insert",
            json!({"table": s(t), "at": 3, "height": 1.0})
        )
        .is_err()
    );
}

#[test]
fn a_field_in_a_cell_updates_when_its_source_changes() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0)).unwrap(),
        )),
    );
    let t = make(
        &mut f,
        space,
        &[30.0],
        data_rows(2, 6.0),
        json!([
            {"cell": "A1", "content": {"kind": "text", "text": "L=", "fields": [
                {"offset": 2, "precision": 1,
                 "source": {"kind": "object", "object": s(line), "path": "length"}}]}},
            formula("A2", "2 * 3")
        ]),
    );
    let node = |f: &Fixture| {
        let view = f
            .document
            .graph()
            .evaluate_now(&table::resolved_node(t).unwrap())
            .unwrap();
        let value = view.result.unwrap();
        value
            .downcast_ref::<table::Resolution>()
            .unwrap()
            .texts
            .clone()
    };
    assert_eq!(node(&f)[&(0, 0)], "L=5.0");
    assert_eq!(node(&f)[&(1, 0)], "6");
    f.set(
        "core.draft.entity.modify",
        &[line],
        "geometry",
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(6.0, 8.0)).unwrap(),
        )),
    );
    assert_eq!(node(&f)[&(0, 0)], "L=10.0");
    // The display list draws the new text.
    let before = list(&f, t).len();
    assert!(before > 0);
    // Deleting the source leaves the marker in the cell and a warning.
    f.run("core.draft.entity.delete", json!({"objects": [s(line)]}))
        .unwrap();
    assert_eq!(
        node(&f)[&(0, 0)],
        format!("L={}", arc_draft::fields::MISSING_MARKER)
    );
    assert!(warnings(&f).contains(&"draft.field.source_missing".to_owned()));
}

#[test]
fn tables_survive_a_package_round_trip() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)).unwrap(),
        )),
    );
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "unit", "objects": [s(line)]}),
    );
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "table", "name": "schedule"}),
    );
    let layer = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let probe = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0)).unwrap(),
        )),
    );
    let t = f.create(
        "core.draft.table.create",
        json!({"owner": s(space), "columns": [20.0, 20.0, 20.0], "style": s(style),
               "rows": rows(&[("title", 6.0), ("header", 5.0), ("data", 5.0), ("data", 5.0)]),
               "max_height": 14.0, "break_gap": 3.0, "position": [1.0, 2.0], "rotation": 0.25,
               "layer": s(layer),
               "cells": [
                   word("A1", "Schedule"),
                   word("A2", "h"),
                   {"cell": "B2", "content": {"kind": "text", "text": "len ", "fields": [
                       {"offset": 4, "source": {"kind": "object", "object": s(probe), "path": "length"}}]}},
                   number("A3", 2.5),
                   formula("B3", "A3 * 4"),
                   {"cell": "C3", "content": {"kind": "block", "block": s(block), "scale": 2.0}},
                   formula("A4", "B4"),
                   formula("B4", "A4")
               ]}),
    );
    f.run(
        "core.draft.table.merge",
        json!({"table": s(t), "range": "A1:C1"}),
    )
    .unwrap();
    let array_block = block;
    f.create(
        "core.draft.array.create",
        json!({"owner": s(space), "block": s(array_block), "position": [0.0, -50.0],
               "array": {"kind": "rectangular", "rows": 2, "columns": 3,
                         "row_spacing": 2.0, "column_spacing": 3.0, "angle": 0.1}}),
    );
    f.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "x", "position": [0.0, 0.0],
               "fields": [{"offset": 1, "source": {"kind": "document", "property": "space_name"}}]}),
    );
    round_trip(&f);
    let before = list(&f, t);
    assert!(
        before.iter().any(|p| p.part == 2),
        "the block cell is drawn"
    );
    assert!(before.iter().any(|p| p.part == 1));
}
