//! The virtualized table through a headless egui context (CORE-UI-013, CORE-UI-014): a source of
//! one million rows is asked only for rows near the viewport, the keyboard moves and scrolls the
//! selection, frozen columns stay at the left, headers report sorts, and copy and paste produce
//! tab-separated text and one paste intent with the refused cells reported.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::cell::RefCell;
use std::ops::Range;

use arc_base::error::Value;
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    ColumnInfo, Intent, Localize, Sort, SortDirection, TableCommands, TableSource, TableView,
};

struct English;
impl Localize for English {
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        let arg = |name: &str| {
            args.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| match v {
                    Value::Text(t) => t.clone(),
                    Value::Int(n) => n.to_string(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default()
        };
        Some(match id {
            "ui-table" => "Table".to_owned(),
            "ui-table-sorted" => format!("{}, sorted {}", arg("name"), arg("direction")),
            "ui-table-sorted-key" => format!(
                "{}, sorted {}, sort key {} of {}",
                arg("name"),
                arg("direction"),
                arg("key"),
                arg("keys")
            ),
            "ui-table-ascending" => "ascending".to_owned(),
            "ui-table-descending" => "descending".to_owned(),
            "ui-table-paste-refused" => format!("{} refused", arg("count")),
            "ui-table-copy-too-large" => "Too many cells".to_owned(),
            "ui-table-too-large" => "Text too large".to_owned(),
            _ => return None,
        })
    }
}

/// A grid of `rows` by `columns` whose cells read `r{row}c{column}`. It records each row window
/// it was asked for, refuses paste into `refuse` (a column) and reports `sorted`.
struct Grid {
    rows: usize,
    columns: usize,
    sorted: Option<Sort>,
    keys: Vec<Sort>,
    refuse: Option<usize>,
    asked: RefCell<Vec<Range<usize>>>,
}

impl Grid {
    fn new(rows: usize, columns: usize) -> Self {
        Self {
            rows,
            columns,
            sorted: None,
            keys: Vec::new(),
            refuse: None,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl TableSource for Grid {
    fn row_count(&self) -> usize {
        self.rows
    }
    fn column_count(&self) -> usize {
        self.columns
    }
    fn column(&self, index: usize) -> Option<ColumnInfo> {
        (index < self.columns).then(|| ColumnInfo {
            name: format!("C{index}"),
            width: 100.0,
            sortable: true,
        })
    }
    fn cells(&self, rows: Range<usize>, columns: Range<usize>) -> Vec<Vec<String>> {
        self.asked.borrow_mut().push(rows.clone());
        rows.map(|r| columns.clone().map(|c| format!("r{r}c{c}")).collect())
            .collect()
    }
    fn sorted(&self) -> Option<Sort> {
        self.sorted
    }
    fn sorted_keys(&self) -> Vec<Sort> {
        if self.keys.is_empty() {
            self.sorted.into_iter().collect()
        } else {
            self.keys.clone()
        }
    }
    fn accepts(&self, _row: usize, column: usize, _text: &str) -> bool {
        self.refuse != Some(column)
    }
}

fn commands() -> TableCommands {
    TableCommands {
        sort: "demo.table.sort".into(),
        paste: "demo.table.paste".into(),
    }
}

struct Pass {
    nodes: Vec<accesskit::Node>,
    commands: Vec<egui::output::OutputCommand>,
}

fn pass(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    ui_fn: impl FnMut(&mut egui::Ui) -> Option<Intent>,
) -> (Pass, Option<Intent>) {
    pass_with(ctx, events, egui::Modifiers::NONE, ui_fn)
}

/// A pass whose held modifiers are `modifiers`; a click reads them from the frame's state.
fn pass_with(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    mut ui_fn: impl FnMut(&mut egui::Ui) -> Option<Intent>,
) -> (Pass, Option<Intent>) {
    // The held modifiers are a state change at the start of the frame, not a field of the input.
    let mut events = events;
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| result = ui_fn(ui));
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    let commands = full.platform_output.commands;
    (Pass { nodes, commands }, result)
}

fn cell(pass: &Pass, text: &str) -> Option<accesskit::Node> {
    pass.nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::Cell && n.label() == Some(text))
        .cloned()
}

fn header(pass: &Pass, text: &str) -> Option<accesskit::Node> {
    pass.nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::ColumnHeader && n.label().is_some_and(|l| l == text))
        .cloned()
}

/// The header of column `column`, whether or not it shows a sort.
fn header_of(pass: &Pass, column: &str) -> Option<accesskit::Node> {
    let prefix = format!("{column},");
    pass.nodes
        .iter()
        .find(|n| {
            n.role() == accesskit::Role::ColumnHeader
                && n.label()
                    .is_some_and(|l| l == column || l.starts_with(&prefix))
        })
        .cloned()
}

fn center(node: &accesskit::Node) -> egui::Pos2 {
    let b = node.bounds().unwrap();
    egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn click(at: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn key(key: egui::Key, shift: bool) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        },
    }
}

fn keys(key_name: egui::Key, count: usize, shift: bool) -> Vec<egui::Event> {
    (0..count).map(|_| key(key_name, shift)).collect()
}

/// Runs one frame of `view` over `source` with `events`.
fn frame(
    ctx: &egui::Context,
    view: &mut TableView,
    source: &dyn TableSource,
    events: Vec<egui::Event>,
) -> (Pass, Option<Intent>) {
    pass(ctx, events, |ui| {
        view.show(ui, source, &commands(), &English)
    })
}

fn key_with(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// A primary click with `modifiers` held, at `at`.
fn click_with(at: egui::Pos2, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

/// The three frames of a drag from `from` to `to`: press, move, release.
fn drag(from: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
    let button = |pos: egui::Pos2, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    [
        vec![egui::Event::PointerMoved(from), button(from, true)],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerMoved(to), button(to, false)],
    ]
}

/// Runs one frame of `view` over `source` with `modifiers` held.
fn frame_with(
    ctx: &egui::Context,
    view: &mut TableView,
    source: &dyn TableSource,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
) -> (Pass, Option<Intent>) {
    pass_with(ctx, events, modifiers, |ui| {
        view.show(ui, source, &commands(), &English)
    })
}

/// Moves column `C0` of a three column table to the right of `C2`, over three frames.
fn move_c0_after_c2(ctx: &egui::Context, view: &mut TableView, source: &Grid, first: &Pass) {
    let from = center(&header(first, "C0").unwrap());
    let [press, moved, release] = drag(from, egui::pos2(260.0, from.y));
    let _ = frame(ctx, view, source, press);
    let _ = frame(ctx, view, source, moved);
    let _ = frame(ctx, view, source, release);
}

fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx
}

#[test]
fn a_million_rows_cost_one_window_of_rows_per_frame() {
    let ctx = new_context();
    let source = Grid::new(1_000_000, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let asked = source.asked.borrow().clone();
    assert!(!asked.is_empty());
    for window in &asked {
        assert!(window.end <= 100, "asked for rows {window:?}");
    }
    assert!(asked.iter().all(|w| w.end - w.start <= 100));

    // End moves the selection to the last row; the next frame fetches only rows near the end.
    source.asked.borrow_mut().clear();
    let (pass, _) = frame(&ctx, &mut view, &source, vec![key(egui::Key::End, false)]);
    let _ = frame(&ctx, &mut view, &source, vec![]);
    assert_eq!(view.cursor().map(|c| c.row), Some(999_999));
    let asked = source.asked.borrow().clone();
    assert!(!asked.is_empty());
    for window in &asked {
        assert!(window.start >= 999_900, "asked for rows {window:?}");
    }
    assert!(cell(&pass, "r0c0").is_none());
}

#[test]
fn keyboard_moves_the_selection_and_scrolls_it_into_view() {
    let ctx = new_context();
    let source = Grid::new(1_000, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").expect("first cell is drawn");
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let _ = frame(&ctx, &mut view, &source, vec![]);

    // Shift+Down 40 times extends the selection; row 40 must now be drawn.
    let (pass, _) = frame(
        &ctx,
        &mut view,
        &source,
        keys(egui::Key::ArrowDown, 40, true),
    );
    let selection = view.selection().expect("a selection exists");
    assert_eq!(selection.rows, 0..41);
    assert_eq!(selection.columns, vec![0]);
    assert!(cell(&pass, "r40c0").is_some(), "row 40 scrolled into view");

    // Page down moves by a page; Home returns to the first row without Shift.
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        vec![key(egui::Key::PageDown, false)],
    );
    let after_page = view.cursor().unwrap().row;
    assert!(after_page > 40);
    let (pass, _) = frame(&ctx, &mut view, &source, vec![key(egui::Key::Home, false)]);
    assert_eq!(view.cursor().map(|c| c.row), Some(0));
    assert!(cell(&pass, "r0c0").is_some());
}

#[test]
fn frozen_columns_stay_at_the_left_when_the_rest_scroll() {
    let ctx = new_context();
    let source = Grid::new(20, 12);
    let mut view = TableView::default();
    view.frozen = 1;
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let frozen_before = cell(&first, "r0c0").unwrap().bounds().unwrap().x0;
    let shifted_before = cell(&first, "r0c2").unwrap().bounds().unwrap().x0;
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let _ = frame(&ctx, &mut view, &source, vec![]);

    let (pass, _) = frame(
        &ctx,
        &mut view,
        &source,
        keys(egui::Key::ArrowRight, 11, false),
    );
    assert_eq!(view.cursor().map(|c| c.column), Some(11));
    let frozen_after = cell(&pass, "r0c0")
        .expect("frozen cell still drawn")
        .bounds()
        .unwrap()
        .x0;
    assert_eq!(frozen_before, frozen_after);
    // Column 2 scrolled out of view; the cursor column is visible instead.
    assert!(cell(&pass, "r0c2").is_none());
    assert!(cell(&pass, "r0c11").is_some());
    assert!(shifted_before > frozen_before);
}

#[test]
fn alt_arrows_resize_the_cursor_column() {
    let ctx = new_context();
    let source = Grid::new(5, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let alt_right = egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::ALT,
    };
    let _ = frame(&ctx, &mut view, &source, vec![alt_right]);
    let _ = frame(&ctx, &mut view, &source, vec![]);
    assert_eq!(view.column_width(0), Some(108.0));
    assert_eq!(view.column_width(1), Some(100.0));
}

#[test]
fn clicking_a_sortable_header_reports_the_sort_and_toggles_direction() {
    let ctx = new_context();
    let mut source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        click(center(&header(&first, "C1").unwrap())),
    );
    let intent = intent.expect("sort intent");
    assert_eq!(intent.command, "demo.table.sort");
    assert_eq!(intent.params["column"], 1);
    assert_eq!(intent.params["direction"], "ascending");

    // The source reports the sort; the header shows it and the next click reverses it.
    source.sorted = Some(Sort {
        column: 1,
        direction: SortDirection::Ascending,
    });
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(header(&pass, "C1, sorted ascending").is_some());
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        click(center(&header(&pass, "C1, sorted ascending").unwrap())),
    );
    assert_eq!(intent.unwrap().params["direction"], "descending");
}

#[test]
fn enter_sorts_the_cursor_column() {
    let ctx = new_context();
    let source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let (_, intent) = frame(&ctx, &mut view, &source, vec![key(egui::Key::Enter, false)]);
    let intent = intent.expect("Enter sorts");
    assert_eq!(intent.params["column"], 0);
}

#[test]
fn copy_puts_the_selected_cells_on_the_clipboard_as_tab_separated_text() {
    let ctx = new_context();
    let source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let _ = frame(&ctx, &mut view, &source, vec![]);
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        keys(egui::Key::ArrowRight, 1, true),
    );
    let (pass, _) = frame(&ctx, &mut view, &source, vec![egui::Event::Copy]);
    let copied: Vec<&String> = pass
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::output::OutputCommand::CopyText(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(copied, vec!["r0c0\tr0c1\n"]);
}

#[test]
fn paste_makes_one_intent_and_reports_refused_cells() {
    let ctx = new_context();
    let mut source = Grid::new(10, 3);
    source.refuse = Some(1);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        vec![egui::Event::Paste("a\tb\nc\t\"d\te\"".into())],
    );
    let intent = intent.expect("accepted cells make a paste intent");
    assert_eq!(intent.command, "demo.table.paste");
    let cells = intent.params["cells"].as_array().unwrap().clone();
    assert_eq!(cells.len(), 2);
    assert_eq!(
        cells[0],
        serde_json::json!({"row": 0, "column": 0, "value": "a"})
    );
    assert_eq!(
        cells[1],
        serde_json::json!({"row": 1, "column": 0, "value": "c"})
    );
    assert_eq!(view.refused_count(), 2);
    assert_eq!(view.refused()[0].column, 1);
}

#[test]
fn a_paste_with_only_refused_cells_emits_no_intent() {
    let ctx = new_context();
    let mut source = Grid::new(2, 1);
    source.refuse = Some(0);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let target = cell(&first, "r0c0").unwrap();
    let _ = frame(&ctx, &mut view, &source, click(center(&target)));
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        vec![egui::Event::Paste("x".into())],
    );
    assert!(intent.is_none());
    assert_eq!(view.refused_count(), 1);
}

/// A grid whose cells all accept in-place edits.
struct Editable(Grid);

impl TableSource for Editable {
    fn row_count(&self) -> usize {
        self.0.row_count()
    }
    fn column_count(&self) -> usize {
        self.0.column_count()
    }
    fn column(&self, index: usize) -> Option<ColumnInfo> {
        self.0.column(index)
    }
    fn cells(&self, rows: Range<usize>, columns: Range<usize>) -> Vec<Vec<String>> {
        self.0.cells(rows, columns)
    }
    fn sorted(&self) -> Option<Sort> {
        self.0.sorted()
    }
    fn accepts(&self, row: usize, column: usize, text: &str) -> bool {
        self.0.accepts(row, column, text)
    }
    fn editable(&self, _row: usize, _column: usize) -> bool {
        true
    }
}

const BAR: f64 = 10.0;

/// Like [`pass`] with an input time, so that double clicks can be recognized.
fn frame_at(
    ctx: &egui::Context,
    time: f64,
    view: &mut TableView,
    source: &dyn TableSource,
    events: Vec<egui::Event>,
) -> (Pass, Option<Intent>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        events,
        time: Some(time),
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| {
        result = view.show(ui, source, &commands(), &English);
    });
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    let commands = full.platform_output.commands;
    (Pass { nodes, commands }, result)
}

fn table_node(pass: &Pass) -> accesskit::Node {
    pass.nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::Table)
        .cloned()
        .expect("the table is drawn")
}

fn select(ctx: &egui::Context, view: &mut TableView, source: &dyn TableSource, text: &str) {
    let (first, _) = frame(ctx, view, source, vec![]);
    let target = cell(&first, text).expect("the cell is drawn");
    let _ = frame(ctx, view, source, click(center(&target)));
    let _ = frame(ctx, view, source, vec![]);
}

fn edit_view() -> TableView {
    let mut view = TableView::default();
    view.edit = Some("demo.table.edit".into());
    view
}

#[test]
fn enter_after_f2_commits_one_edit_intent_with_row_column_and_value() {
    let ctx = new_context();
    let source = Editable(Grid::new(10, 3));
    let mut view = edit_view();
    select(&ctx, &mut view, &source, "r2c1");
    let _ = frame(&ctx, &mut view, &source, vec![key(egui::Key::F2, false)]);
    assert!(view.is_editing());
    let (_, intent) = frame(&ctx, &mut view, &source, vec![key(egui::Key::Enter, false)]);
    let intent = intent.expect("Enter commits the edit");
    assert_eq!(intent.command, "demo.table.edit");
    assert_eq!(intent.params["row"], 2);
    assert_eq!(intent.params["column"], 1);
    assert_eq!(intent.params["value"], "r2c1");
    assert!(!view.is_editing());
}

#[test]
fn escape_cancels_an_edit_without_an_intent() {
    let ctx = new_context();
    let source = Editable(Grid::new(10, 3));
    let mut view = edit_view();
    select(&ctx, &mut view, &source, "r0c0");
    let _ = frame(&ctx, &mut view, &source, vec![key(egui::Key::F2, false)]);
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        vec![key(egui::Key::Escape, false)],
    );
    assert!(intent.is_none());
    assert!(!view.is_editing());
}

#[test]
fn a_double_click_starts_an_edit_and_cells_the_source_refuses_do_not_edit() {
    let ctx = new_context();
    let source = Editable(Grid::new(10, 3));
    let mut view = edit_view();
    let (first, _) = frame_at(&ctx, 0.0, &mut view, &source, vec![]);
    let target = center(&cell(&first, "r1c2").unwrap());
    let _ = frame_at(&ctx, 1.0, &mut view, &source, click(target));
    let _ = frame_at(&ctx, 1.1, &mut view, &source, click(target));
    assert!(view.is_editing(), "the double click opens the editor");
    let (_, intent) = frame_at(
        &ctx,
        1.2,
        &mut view,
        &source,
        vec![key(egui::Key::Enter, false)],
    );
    assert_eq!(intent.expect("edit").params["value"], "r1c2");

    // A source that marks nothing editable never opens the editor, even with F2.
    let fixed = Grid::new(10, 3);
    let mut view = edit_view();
    select(&ctx, &mut view, &fixed, "r0c0");
    let _ = frame(&ctx, &mut view, &fixed, vec![key(egui::Key::F2, false)]);
    assert!(!view.is_editing());
}

#[test]
fn enter_on_an_editable_cell_edits_instead_of_sorting() {
    let ctx = new_context();
    let source = Editable(Grid::new(10, 3));
    let mut view = edit_view();
    select(&ctx, &mut view, &source, "r0c0");
    let (_, intent) = frame(&ctx, &mut view, &source, vec![key(egui::Key::Enter, false)]);
    assert!(view.is_editing());
    assert!(intent.is_none());
}

#[test]
fn horizontal_wheel_and_shift_wheel_scroll_the_scrolling_columns() {
    let ctx = new_context();
    let source = Grid::new(3, 12);
    let mut view = TableView::default();
    select(&ctx, &mut view, &source, "r0c0");
    let at = center(&cell(&frame(&ctx, &mut view, &source, vec![]).0, "r0c0").unwrap());
    let wheel = |delta: egui::Vec2, modifiers: egui::Modifiers| egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers,
    };
    let (pass, _) = frame(
        &ctx,
        &mut view,
        &source,
        vec![
            egui::Event::PointerMoved(at),
            wheel(egui::vec2(-300.0, 0.0), egui::Modifiers::NONE),
        ],
    );
    assert!(cell(&pass, "r0c9").is_some(), "the wheel scrolled right");
    assert!(cell(&pass, "r0c2").is_none());

    let mut view = TableView::default();
    select(&ctx, &mut view, &source, "r0c0");
    let (pass, _) = frame(
        &ctx,
        &mut view,
        &source,
        vec![
            egui::Event::PointerMoved(at),
            wheel(egui::vec2(0.0, -300.0), egui::Modifiers::SHIFT),
        ],
    );
    assert!(
        cell(&pass, "r0c9").is_some(),
        "Shift with the wheel scrolls right"
    );
}

#[test]
fn the_horizontal_scroll_bar_moves_the_columns_when_clicked() {
    let ctx = new_context();
    let source = Grid::new(3, 12);
    let mut view = TableView::default();
    select(&ctx, &mut view, &source, "r0c0");
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    let bar = pass
        .nodes
        .iter()
        .find(|n| {
            n.role() == accesskit::Role::ScrollBar
                && n.label() == Some("ui-table-scroll-horizontal")
        })
        .cloned()
        .expect("the horizontal scroll bar is named");
    let b = bar.bounds().unwrap();
    let at = egui::pos2((b.x1 - 3.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    let _ = frame(&ctx, &mut view, &source, click(at));
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(cell(&pass, "r0c11").is_some(), "the last column is in view");
}

#[test]
fn row_50_000_000_is_positioned_exactly_at_the_bottom_of_the_body() {
    let ctx = new_context();
    let source = Grid::new(50_000_000, 2);
    let mut view = TableView::default();
    select(&ctx, &mut view, &source, "r0c0");
    let _ = frame(&ctx, &mut view, &source, vec![key(egui::Key::End, false)]);
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert_eq!(view.cursor().map(|c| c.row), Some(49_999_999));
    let last = cell(&pass, "r49999999c0").expect("the last row is drawn");
    let bounds = last.bounds().unwrap();
    let body_bottom = table_node(&pass).bounds().unwrap().y1 - BAR;
    assert!(
        (bounds.y1 - body_bottom).abs() < 1e-3,
        "last row ends at the body bottom: {} vs {body_bottom}",
        bounds.y1
    );
    assert!(cell(&pass, "r0c0").is_none());
}

#[test]
fn right_to_left_puts_the_first_and_frozen_columns_at_the_right_edge() {
    let ctx = new_context();
    let source = Grid::new(4, 3);
    let mut view = TableView::default();
    view.frozen = 1;
    view.language = "ar".into();
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    let table = table_node(&pass).bounds().unwrap();
    let frozen = cell(&pass, "r0c0").unwrap().bounds().unwrap();
    let next = cell(&pass, "r0c1").unwrap().bounds().unwrap();
    assert!(frozen.x0 > next.x0, "the frozen column is at the right");
    assert!((frozen.x1 - (table.x1 - BAR)).abs() < 1e-3);

    let mut view = TableView::default();
    view.frozen = 1;
    view.language = "en".into();
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    let frozen = cell(&pass, "r0c0").unwrap().bounds().unwrap();
    let next = cell(&pass, "r0c1").unwrap().bounds().unwrap();
    assert!(
        frozen.x0 < next.x0,
        "left to right keeps the frozen column left"
    );
}

/// The painted text of cell `text`, with the x coordinate where its galley starts and ends.
fn painted_text_span(full: &egui::FullOutput, text: &str) -> (f32, f32) {
    full.shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => {
                Some((shape.pos.x, shape.pos.x + shape.galley.size().x))
            }
            _ => None,
        })
        .expect("the cell text is painted")
}

#[test]
fn cell_text_starts_at_the_logical_start_of_its_cell() {
    for (language, rtl) in [("ar", true), ("en", false)] {
        let ctx = new_context();
        let source = Grid::new(2, 2);
        let mut view = TableView::default();
        view.language = language.into();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input, |ui| {
            let _ = view.show(ui, &source, &commands(), &English);
        });
        full.textures_delta.clear();
        let nodes: Vec<accesskit::Node> = full
            .platform_output
            .accesskit_update
            .as_ref()
            .map(|u| u.nodes.iter().map(|(_, n)| n.clone()).collect())
            .unwrap_or_default();
        let pass = Pass {
            nodes,
            commands: Vec::new(),
        };
        let bounds = cell(&pass, "r0c1").unwrap().bounds().unwrap();
        let (start, end) = painted_text_span(&full, "r0c1");
        if rtl {
            assert!(
                (f64::from(end) - (bounds.x1 - 6.0)).abs() < 0.5,
                "right to left: the text ends at the cell's right edge"
            );
        } else {
            assert!(
                (f64::from(start) - (bounds.x0 + 6.0)).abs() < 0.5,
                "left to right: the text starts at the cell's left edge"
            );
        }
    }
}

/// The `sort` intent's `keys` parameter as (column, direction) pairs.
fn sort_keys_of(intent: &Intent) -> Vec<(u64, String)> {
    intent.params["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["column"].as_u64().unwrap(),
                k["direction"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn dragging_a_header_moves_the_column_and_its_cells_follow() {
    let ctx = new_context();
    let source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    move_c0_after_c2(&ctx, &mut view, &source, &first);
    assert_eq!(view.column_order(), &[1, 2, 0]);
    let (pass, intent) = frame(&ctx, &mut view, &source, vec![]);
    assert!(intent.is_none(), "a drag is not a sort");
    let x = |text: &str| center(&cell(&pass, text).unwrap()).x;
    assert!(x("r0c1") < x("r0c2") && x("r0c2") < x("r0c0"));
}

#[test]
fn alt_shift_arrows_move_the_cursor_column_and_alt_alone_still_resizes() {
    let ctx = new_context();
    let source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        click(center(&cell(&first, "r0c0").unwrap())),
    );
    let _ = frame(&ctx, &mut view, &source, vec![]);
    let alt_shift = egui::Modifiers::ALT | egui::Modifiers::SHIFT;
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        vec![key_with(egui::Key::ArrowRight, alt_shift)],
    );
    assert_eq!(view.column_order(), &[1, 0, 2]);
    assert_eq!(
        view.cursor().unwrap().column,
        0,
        "the cursor stays on its column"
    );
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        vec![key_with(egui::Key::ArrowLeft, alt_shift)],
    );
    assert_eq!(view.column_order(), &[0, 1, 2]);
    // The move is not taken past either end of the order.
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        vec![key_with(egui::Key::ArrowLeft, alt_shift)],
    );
    assert_eq!(view.column_order(), &[0, 1, 2]);
    // Alt without Shift still resizes the cursor column and leaves the order alone.
    let before = view.column_width(0).unwrap();
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        vec![key_with(egui::Key::ArrowRight, egui::Modifiers::ALT)],
    );
    assert_eq!(view.column_width(0), Some(before + 8.0));
    assert_eq!(view.column_order(), &[0, 1, 2]);
}

#[test]
fn copy_and_paste_follow_the_column_order() {
    let ctx = new_context();
    let source = Grid::new(10, 3);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    move_c0_after_c2(&ctx, &mut view, &source, &first);
    let (moved, _) = frame(&ctx, &mut view, &source, vec![]);
    // Select r0c2 and extend to the right: the selection runs over the drawn order, [2, 0].
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        click(center(&cell(&moved, "r0c2").unwrap())),
    );
    let _ = frame(&ctx, &mut view, &source, vec![]);
    let _ = frame(
        &ctx,
        &mut view,
        &source,
        keys(egui::Key::ArrowRight, 1, true),
    );
    assert_eq!(view.selection().unwrap().columns, vec![2, 0]);
    let (pass, _) = frame(&ctx, &mut view, &source, vec![egui::Event::Copy]);
    let copied: Vec<&String> = pass
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::output::OutputCommand::CopyText(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(copied, vec!["r0c2\tr0c0\n"]);

    // A paste starts at the selection's left corner and fills the drawn columns to the right.
    let (_, intent) = frame(
        &ctx,
        &mut view,
        &source,
        vec![egui::Event::Paste("a\tb\n".to_owned())],
    );
    let cells = intent.expect("a paste intent").params["cells"].clone();
    assert_eq!(
        cells,
        serde_json::json!([
            {"row": 0, "column": 2, "value": "a"},
            {"row": 0, "column": 0, "value": "b"},
        ])
    );
}

#[test]
fn a_plain_click_sets_one_key_and_a_modified_click_adds_and_toggles_keys() {
    let ctx = new_context();
    let source = Grid::new(10, 5);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let ctrl = egui::Modifiers::COMMAND;
    let at = |pass: &Pass, column: &str| center(&header_of(pass, column).unwrap());

    let (_, intent) = frame(&ctx, &mut view, &source, click(at(&first, "C1")));
    let intent = intent.expect("sort intent");
    assert_eq!(sort_keys_of(&intent), vec![(1, "ascending".to_owned())]);

    let (_, intent) = frame_with(
        &ctx,
        &mut view,
        &source,
        click_with(at(&first, "C0"), ctrl),
        ctrl,
    );
    let intent = intent.expect("a modified click adds a key");
    assert_eq!(intent.params["column"], 0);
    assert_eq!(
        sort_keys_of(&intent),
        vec![(1, "ascending".to_owned()), (0, "ascending".to_owned())]
    );
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(header(&pass, "C1, sorted ascending, sort key 1 of 2").is_some());
    assert!(header(&pass, "C0, sorted ascending, sort key 2 of 2").is_some());

    // A modified click on a key toggles it in place, keeping its place in the keys.
    let (_, intent) = frame_with(
        &ctx,
        &mut view,
        &source,
        click_with(at(&pass, "C1"), ctrl),
        ctrl,
    );
    let intent = intent.expect("toggle");
    assert_eq!(intent.params["direction"], "descending");
    assert_eq!(
        sort_keys_of(&intent),
        vec![(1, "descending".to_owned()), (0, "ascending".to_owned())]
    );

    // A plain click sets one key: C0 is ascending, so it turns descending and the others go.
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    let (_, intent) = frame(&ctx, &mut view, &source, click(at(&pass, "C0")));
    let intent = intent.expect("plain click");
    assert_eq!(sort_keys_of(&intent), vec![(0, "descending".to_owned())]);
    assert_eq!(view.sort_keys().len(), 1);
}

#[test]
fn the_key_limit_refuses_a_new_key_without_an_intent_and_still_toggles_keys() {
    let ctx = new_context();
    let source = Grid::new(10, 5);
    let mut view = TableView::default();
    let (first, _) = frame(&ctx, &mut view, &source, vec![]);
    let ctrl = egui::Modifiers::COMMAND;
    let at = |pass: &Pass, column: &str| center(&header_of(pass, column).unwrap());
    let _ = frame(&ctx, &mut view, &source, click(at(&first, "C1")));
    for column in ["C0", "C2", "C3"] {
        let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
        let _ = frame_with(
            &ctx,
            &mut view,
            &source,
            click_with(at(&pass, column), ctrl),
            ctrl,
        );
    }
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert_eq!(view.sort_keys().len(), 4);
    let (_, refused) = frame_with(
        &ctx,
        &mut view,
        &source,
        click_with(at(&pass, "C4"), ctrl),
        ctrl,
    );
    assert!(refused.is_none(), "a fifth key gives no intent");
    assert_eq!(view.sort_keys().len(), 4);
    assert!(view.sort_keys().iter().all(|k| k.column != 4));

    let (_, toggled) = frame_with(
        &ctx,
        &mut view,
        &source,
        click_with(at(&pass, "C0"), ctrl),
        ctrl,
    );
    let toggled = toggled.expect("an existing key still toggles at the limit");
    assert_eq!(sort_keys_of(&toggled).len(), 4);
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(header(&pass, "C0, sorted descending, sort key 2 of 4").is_some());
}

#[test]
fn keys_are_seeded_from_the_source_and_survive_a_refresh_until_their_column_vanishes() {
    let ctx = new_context();
    let mut source = Grid::new(10, 3);
    source.keys = vec![
        Sort {
            column: 2,
            direction: SortDirection::Ascending,
        },
        Sort {
            column: 1,
            direction: SortDirection::Descending,
        },
    ];
    let mut view = TableView::default();
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(header(&pass, "C2, sorted ascending, sort key 1 of 2").is_some());
    assert!(header(&pass, "C1, sorted descending, sort key 2 of 2").is_some());

    // Rows change: the keys stay.
    source.rows = 20;
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert!(header(&pass, "C2, sorted ascending, sort key 1 of 2").is_some());

    // Column 2 vanishes: its key goes, the other key stays and now stands alone.
    source.columns = 2;
    let (pass, _) = frame(&ctx, &mut view, &source, vec![]);
    assert_eq!(view.sort_keys().len(), 1);
    assert!(header(&pass, "C1, sorted descending").is_some());
    let (_, intent) = frame_with(
        &ctx,
        &mut view,
        &source,
        click_with(
            center(&header(&pass, "C0").unwrap()),
            egui::Modifiers::COMMAND,
        ),
        egui::Modifiers::COMMAND,
    );
    let intent = intent.expect("a modified click");
    assert_eq!(
        sort_keys_of(&intent),
        vec![(1, "descending".to_owned()), (0, "ascending".to_owned())]
    );
}
