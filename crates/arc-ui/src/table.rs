//! The virtualized table (CORE-UI-013) and the table clipboard (CORE-UI-014).
//!
//! A [`TableView`] draws a [`TableSource`]. The source answers row and column counts, column
//! descriptions and cell texts for a window of rows and a span of columns; the view asks only
//! for the rows inside the viewport plus [`MARGIN`] rows on each side, so a frame costs the same
//! for a million rows as for a hundred. Leading columns named by [`TableView::frozen`] stay at
//! the logical start while the others scroll horizontally.
//!
//! The vertical position is an integer index of the first visible row and a small offset in
//! points below it, so a row far down a table of tens of millions of rows is placed exactly.
//! Rows are laid out relative to the first visible row, never at an absolute position.
//!
//! The view never sorts or edits data itself. A click on a sortable header, or Enter on the
//! cursor column, returns the intent of [`TableCommands::sort`] with the table's sort keys
//! ([`TableView::sort_keys`]): a plain click sets one key, a click with Ctrl or Cmd adds or toggles
//! a secondary key. Dragging a header, or Alt with Shift and the horizontal arrows, moves a column
//! in the order ([`TableView::column_order`]). An in-place edit (Enter, F2 or a
//! double click on a cell the source reports through [`TableSource::editable`]) shows a text field
//! over the cell; Enter returns one intent of the edit command named by [`TableView::edit`] with
//! `row`, `column` and `value`, and Escape cancels. A paste of tab-separated text returns one
//! intent of [`TableCommands::paste`] with the accepted cells (`cells`, each with `row`, `column`
//! and `value`); the source decides per cell through [`TableSource::accepts`], and refused cells
//! are kept in [`TableView::refused`].
//!
//! Copy and paste use the events egui delivers to a focused widget: `Event::Copy` puts the
//! selected cells on the clipboard through [`egui::Context::copy_text`], and `Event::Paste` carries
//! the text that [`parse_grid`] reads. [`format_grid`] and [`parse_grid`] are pure and inverse to
//! each other for grids without empty rows.
//!
//! In a right-to-left layout ([`crate::text::Direction`]) the first column is at the right edge,
//! the frozen columns stay at the right, text aligns to the right, and the Left and Right arrows
//! move in the visual direction.

use std::ops::Range;

use arc_base::error::Value;
use arc_base::{Error, Result};
use egui::{
    CursorIcon, Event, FontId, Id, Key, Modifiers, MouseWheelUnit, Pos2, Rect, Role, Sense,
    StrokeKind, TextEdit, TextStyle, Ui, Vec2, WidgetInfo, pos2, vec2,
};

use crate::command::Intent;
use crate::error::codes;
use crate::table_model::{self, MAX_SORT_KEYS};
use crate::text::{Direction, Localize, Message, messages, resolve};

/// The most columns shown; the columns beyond are not drawn.
pub const MAX_COLUMNS: usize = 256;
/// Rows fetched above and below the viewport.
pub const MARGIN: usize = 2;
/// Columns fetched beside the visible span.
pub const COLUMN_MARGIN: usize = 1;
/// The narrowest column, in logical points.
pub const MIN_WIDTH: f32 = 24.0;
/// The widest column, in logical points.
pub const MAX_WIDTH: f32 = 2_000.0;
/// The width of a column the source does not describe, in logical points.
pub const DEFAULT_WIDTH: f32 = 120.0;
/// The change of a column width by one keyboard step, in logical points.
pub const RESIZE_STEP: f32 = 8.0;
/// The width of the grip that resizes a column, in logical points.
pub const GRIP_WIDTH: f32 = 6.0;
/// The thickness of the scroll bars at the right and bottom edges, in logical points.
pub const BAR_WIDTH: f32 = 10.0;
/// Characters drawn per cell.
pub const MAX_CELL_CHARS: usize = 256;
/// The most cells a copy puts on the clipboard.
pub const MAX_COPY_CELLS: usize = 100_000;
/// Refused cells kept for the application after a paste.
pub const MAX_REPORTED_REFUSALS: usize = 100;
/// The most bytes of clipboard text a paste reads.
pub const MAX_PASTE_BYTES: usize = 1 << 20;
/// The most cells a paste reads.
pub const MAX_PASTE_CELLS: usize = 1_000_000;

/// The direction of a sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

impl SortDirection {
    /// The parameter text of the direction: `ascending` or `descending`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// A sort the source applies: a column index and a direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    /// The column index.
    pub column: usize,
    /// The direction.
    pub direction: SortDirection,
}

/// One column as the source describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    /// The header text (user data, not a message).
    pub name: String,
    /// The initial width in logical points; clamped to [`MIN_WIDTH`] and [`MAX_WIDTH`].
    pub width: f32,
    /// A click on the header reports a sort.
    pub sortable: bool,
}

/// A table's data, read by the view for the rows and columns it draws.
pub trait TableSource {
    /// The number of rows.
    fn row_count(&self) -> usize;
    /// The number of columns.
    fn column_count(&self) -> usize;
    /// The description of column `index`, or `None` when it does not exist.
    fn column(&self, index: usize) -> Option<ColumnInfo>;
    /// The texts of the cells in `rows` and `columns`: one entry per row of `rows`, each holding
    /// one text per column of `columns`. The view calls this only for the rows in view, with a
    /// margin, and for the columns in view and the frozen ones. After a reorder those columns
    /// are not consecutive, so the view makes one call per run of consecutive indices.
    fn cells(&self, rows: Range<usize>, columns: Range<usize>) -> Vec<Vec<String>>;
    /// The primary sort the source applies. See [`TableSource::sorted_keys`].
    fn sorted(&self) -> Option<Sort>;
    /// The sort keys the source applies, first key first. The view starts with these keys when
    /// it has none, and then keeps its own keys, which the user changes with the header. The
    /// default is [`TableSource::sorted`] alone.
    fn sorted_keys(&self) -> Vec<Sort> {
        self.sorted().into_iter().collect()
    }
    /// Whether cell (`row`, `column`) accepts `text` from a paste.
    fn accepts(&self, row: usize, column: usize, text: &str) -> bool;
    /// Whether cell (`row`, `column`) can be edited in place. The default is read-only.
    fn editable(&self, _row: usize, _column: usize) -> bool {
        false
    }
}

/// The intent names of a table: `sort` takes `column` and `direction` (the key the click set) and
/// `keys`, an array of objects with `column` and `direction`, the table's keys after the click,
/// first key first; `paste` takes `cells`, an array of objects with `row`, `column` and `value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCommands {
    /// The sort command.
    pub sort: String,
    /// The paste command.
    pub paste: String,
}

/// A cell by row and column index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellRef {
    /// The row index.
    pub row: usize,
    /// The column index.
    pub column: usize,
}

/// The selected cells: a range of rows and the selected column indices, in drawn order from the
/// logical start. Columns are not contiguous in the source after a reorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The selected rows.
    pub rows: Range<usize>,
    /// The selected columns, in the order the table draws them.
    pub columns: Vec<usize>,
}

impl Selection {
    /// Whether `cell` is inside the selection.
    #[must_use]
    pub fn contains(&self, cell: CellRef) -> bool {
        self.rows.contains(&cell.row) && self.columns.contains(&cell.column)
    }
}

/// A cell being edited in place: its text so far, and whether the editor still needs focus.
#[derive(Debug, Clone)]
struct Editing {
    cell: CellRef,
    text: String,
    focus: bool,
}

/// The table's view state: frozen columns, column widths, scroll positions, the selection, the
/// cell being edited and the report of the last paste.
#[derive(Debug, Clone, Default)]
pub struct TableView {
    /// Leading columns that stay at the logical start while the others scroll.
    pub frozen: usize,
    /// The language tag of the table's text; its direction sets the layout direction.
    pub language: String,
    /// Overrides the direction the language gives.
    pub direction: Option<Direction>,
    /// The command an in-place edit emits, with `row`, `column` and `value`. `None` disables
    /// in-place editing.
    pub edit: Option<String>,
    widths: Vec<f32>,
    /// The source column index drawn at each visual position, from the logical start.
    order: Vec<usize>,
    /// The sort keys, first key first.
    sort: Vec<Sort>,
    /// The column whose header is being dragged.
    reorder: Option<usize>,
    /// Horizontal scroll in logical points from the first scrolling column's left edge.
    scroll_x: f32,
    /// The first row at the top of the body.
    scroll_row: usize,
    /// How far the first row is scrolled up, in points: `[0, row height)` after clamping.
    scroll_offset: f32,
    anchor: Option<CellRef>,
    cursor: Option<CellRef>,
    refused: Vec<CellRef>,
    refused_count: usize,
    notice: Option<Message>,
    editing: Option<Editing>,
}

/// Commands of the table's keyboard navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nav {
    Rows(isize),
    Columns(isize),
    Pages(isize),
    Home,
    End,
}

/// The geometry of one frame, in screen coordinates except for `offsets`.
struct Geometry {
    /// The whole table, including the header and the scroll bars.
    rect: Rect,
    /// The header row over the columns.
    header: Rect,
    /// The rows and columns, without the scroll bars.
    body: Rect,
    rows: usize,
    columns: usize,
    frozen: usize,
    /// Left edge of each column in the table's own coordinates, plus the right edge of the last.
    offsets: Vec<f32>,
    row_height: f32,
    frozen_width: f32,
    /// The width of the body that the scrolling columns share.
    scroll_width: f32,
    rtl: bool,
}

impl Geometry {
    fn body_height(&self) -> f32 {
        self.body.height()
    }

    fn total_width(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    fn scrollable_width(&self) -> f32 {
        self.total_width() - self.frozen_width
    }

    /// The whole rows that fit in the body, at least one.
    fn rows_fit(&self) -> usize {
        f64_to_usize((self.body_height() / self.row_height).floor()).max(1)
    }
}

impl TableView {
    /// The selected cells, or `None` when no cell is selected.
    #[must_use]
    pub fn selection(&self) -> Option<Selection> {
        let (anchor, cursor) = (self.anchor?, self.cursor?);
        let (from, to) = (self.visual(anchor.column)?, self.visual(cursor.column)?);
        Some(Selection {
            rows: span(anchor.row, cursor.row),
            columns: self.order.get(span(from, to))?.to_vec(),
        })
    }

    /// The source column indices in the order the table draws them, from the logical start. Empty
    /// until the view has drawn the table once.
    #[must_use]
    pub fn column_order(&self) -> &[usize] {
        &self.order
    }

    /// Moves the column at visual position `from` so that it ends at position `to`. Positions
    /// count the drawn order from the logical start.
    ///
    /// # Errors
    ///
    /// `ui.table.column_out_of_range` when a position is not a drawn column.
    pub fn move_column(&mut self, from: usize, to: usize) -> Result<()> {
        table_model::move_column(&mut self.order, from, to)
    }

    /// The sort keys, first key first.
    #[must_use]
    pub fn sort_keys(&self) -> &[Sort] {
        &self.sort
    }

    /// The cell with the keyboard focus.
    #[must_use]
    pub fn cursor(&self) -> Option<CellRef> {
        self.cursor
    }

    /// The width of source column `index`, once the view has drawn the table.
    #[must_use]
    pub fn column_width(&self, index: usize) -> Option<f32> {
        self.widths.get(index).copied()
    }

    /// The cells the last paste refused, at most [`MAX_REPORTED_REFUSALS`] of them.
    #[must_use]
    pub fn refused(&self) -> &[CellRef] {
        &self.refused
    }

    /// The number of cells the last paste refused, including out-of-range cells.
    #[must_use]
    pub fn refused_count(&self) -> usize {
        self.refused_count
    }

    /// Whether a cell is being edited in place.
    #[must_use]
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The layout direction: [`TableView::direction`] when set, else the direction of
    /// [`TableView::language`].
    #[must_use]
    pub fn layout_direction(&self) -> Direction {
        Direction::resolve(&self.language, self.direction)
    }

    fn sync_widths(&mut self, source: &dyn TableSource, columns: usize) {
        for index in self.widths.len()..columns {
            let width = source.column(index).map_or(DEFAULT_WIDTH, |c| c.width);
            self.widths.push(clamp_width(width));
        }
    }

    /// The source column drawn at visual position `at`.
    fn logical(&self, at: usize) -> Option<usize> {
        self.order.get(at).copied()
    }

    /// The visual position of source column `column`.
    fn visual(&self, column: usize) -> Option<usize> {
        self.order.iter().position(|&c| c == column)
    }

    /// The width of the column drawn at visual position `at`.
    fn width_at(&self, at: usize) -> f32 {
        self.logical(at)
            .and_then(|c| self.widths.get(c).copied())
            .unwrap_or(DEFAULT_WIDTH)
    }

    /// Drops the keys of columns the source no longer has, and takes the keys the source applies
    /// when the view has none yet.
    fn sync_sort(&mut self, source: &dyn TableSource) {
        table_model::retain_sort_keys(&mut self.sort, |c| source.column(c).is_some());
        if self.sort.is_empty() {
            for key in source.sorted_keys() {
                let known = source.column(key.column).is_some();
                let new = !self.sort.iter().any(|s| s.column == key.column);
                if known && new && self.sort.len() < MAX_SORT_KEYS {
                    self.sort.push(key);
                }
            }
        }
    }

    /// The position a column dragged to pointer position `x` lands at, in the order without the
    /// column itself: the number of other columns that lie before the pointer on the logical
    /// start side.
    fn drop_position(&self, g: &Geometry, from: usize, x: f32) -> usize {
        (0..g.columns)
            .filter(|&at| at != from)
            .filter(|&at| {
                let center = self.column_left(g, at) + self.width_at(at) / 2.0;
                if g.rtl { center > x } else { center < x }
            })
            .count()
    }

    /// Moves source column `column` to the position under pointer position `x`.
    fn drop_column(&mut self, g: &Geometry, column: usize, x: f32) {
        let Some(from) = self.visual(column) else {
            return;
        };
        let to = self.drop_position(g, from, x);
        // `to` counts the other columns, so it is a position of the order: the move is accepted.
        let _ = self.move_column(from, to);
    }

    /// The intent of a click on the header of `column`, or of Enter on its cell. A plain click
    /// sets the one key; an additive one adds or toggles a key. A refused key gives no intent.
    fn sort_click(
        &mut self,
        source: &dyn TableSource,
        commands: &TableCommands,
        column: usize,
        additive: bool,
    ) -> Option<Intent> {
        let columns = source.column_count().min(MAX_COLUMNS);
        let clicked = if additive {
            table_model::sort_additive(&mut self.sort, columns, column)
        } else {
            table_model::sort_single(&mut self.sort, columns, column)
        };
        Some(sort_intent(commands, clicked.ok()?, &self.sort))
    }

    fn click(&mut self, cell: CellRef, extend: bool) {
        if extend {
            self.anchor = self.anchor.or(self.cursor).or(Some(cell));
            self.cursor = Some(cell);
        } else {
            self.anchor = Some(cell);
            self.cursor = Some(cell);
        }
    }

    /// Draws the table in the space left in `ui` and returns the intent of a sort, an edit or a
    /// paste.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        source: &dyn TableSource,
        commands: &TableCommands,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let rows = source.row_count();
        let columns = source.column_count().min(MAX_COLUMNS);
        self.sync_widths(source, columns);
        self.order = table_model::reconcile_order(&self.order, columns);
        self.sync_sort(source);
        self.clamp_selection(rows, columns);
        self.clamp_editing(rows, columns);
        self.show_notices(ui, localize);

        let row_height = ui.spacing().interact_size.y.max(1.0);
        let size = vec2(
            ui.available_width().max(1.0),
            ui.available_height().max(row_height * 4.0),
        );
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let id = ui.scope_id().with("arc-table");
        let container = ui.interact(rect, id, Sense::click());
        let label = messages::TABLE.text(localize);
        container.widget_info(|| WidgetInfo::labeled(Role::Table, true, label.as_str()));
        if container.clicked() && self.editing.is_none() {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }

        let rtl = self.layout_direction().is_right_to_left();
        let g = self.geometry(rect, rows, columns, row_height, rtl);
        self.settle(&g);
        self.wheel(ui, &g);
        self.settle(&g);
        let mut intent = None;
        if container.has_focus() {
            intent = self.keyboard(ui, source, commands, &g);
            self.settle(&g);
        }
        intent = intent.or(self.header(ui, source, commands, &g, id, localize));
        self.body(ui, source, &g, id);
        intent = intent.or(self.editor(ui, &g, id, localize));
        self.scrollbars(ui, &g, id, localize);
        intent
    }

    fn geometry(
        &self,
        rect: Rect,
        rows: usize,
        columns: usize,
        row_height: f32,
        rtl: bool,
    ) -> Geometry {
        let widths: Vec<f32> = (0..columns).map(|at| self.width_at(at)).collect();
        let offsets = column_offsets(&widths);
        let frozen = self.frozen.min(columns);
        let frozen_width = offsets.get(frozen).copied().unwrap_or(0.0);
        let total = offsets.last().copied().unwrap_or(0.0);
        let right = (rect.right() - BAR_WIDTH).max(rect.left());
        let top = rect.top() + row_height;
        let bottom = (rect.bottom() - BAR_WIDTH).max(top);
        let body = Rect::from_min_max(pos2(rect.left(), top), pos2(right, bottom));
        let header = Rect::from_min_max(pos2(rect.left(), rect.top()), pos2(right, top));
        Geometry {
            rect,
            header,
            body,
            rows,
            columns,
            frozen,
            offsets,
            row_height,
            frozen_width,
            scroll_width: (body.width() - frozen_width).max(0.0).min(total),
            rtl,
        }
    }

    /// The largest horizontal scroll: the scrollable columns beyond the body's width.
    fn max_scroll_x(&self, g: &Geometry) -> f32 {
        (g.scrollable_width() - g.scroll_width).max(0.0)
    }

    /// Keeps both scroll positions and the cursor's row inside the table.
    fn settle(&mut self, g: &Geometry) {
        self.scroll_x = clamp_offset(self.scroll_x, self.max_scroll_x(g));
        let (max_row, max_offset) = bottom(g.rows, g.body_height(), g.row_height);
        if !self.scroll_offset.is_finite() {
            self.scroll_offset = 0.0;
        }
        self.scroll_offset = self.scroll_offset.clamp(0.0, g.row_height);
        if self.scroll_row > max_row {
            self.scroll_row = max_row;
            self.scroll_offset = max_offset;
        }
        if self.scroll_row == max_row {
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }
    }

    /// Scrolls the rows by `dy` points (positive moves the content down, showing rows above).
    fn scroll_rows(&mut self, g: &Geometry, dy: f32) {
        if !dy.is_finite() {
            return;
        }
        let h = g.row_height;
        let total = self.scroll_offset - dy;
        let steps = (total / h).floor();
        let whole = steps_of(steps);
        let row = self.scroll_row.cast_signed() + whole;
        if row < 0 {
            self.scroll_row = 0;
            self.scroll_offset = 0.0;
        } else {
            self.scroll_row = usize::try_from(row).unwrap_or(0);
            self.scroll_offset = (total - steps * h).clamp(0.0, h);
        }
        self.settle(g);
    }

    /// Applies wheel input over the table: vertical wheel moves rows, horizontal wheel (or Shift
    /// with the vertical wheel) moves the scrolling columns. The events are consumed.
    fn wheel(&mut self, ui: &Ui, g: &Geometry) {
        if !ui.rect_contains_pointer(g.rect) {
            return;
        }
        for event in take_events(ui, |e| matches!(e, Event::MouseWheel { .. })) {
            let Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            let scale = match unit {
                MouseWheelUnit::Point => 1.0,
                MouseWheelUnit::Line => g.row_height,
                MouseWheelUnit::Page => g.body_height(),
            };
            let delta: Vec2 = delta * scale;
            let (dx, dy) = if modifiers.shift {
                (if delta.x != 0.0 { delta.x } else { delta.y }, 0.0)
            } else {
                (delta.x, delta.y)
            };
            if dx.is_finite() {
                self.scroll_x = clamp_offset(self.scroll_x - dx, self.max_scroll_x(g));
            }
            self.scroll_rows(g, dy);
        }
    }

    fn clamp_selection(&mut self, rows: usize, columns: usize) {
        self.anchor = self.anchor.and_then(|c| clamp_cell(c, rows, columns));
        self.cursor = self.cursor.and_then(|c| clamp_cell(c, rows, columns));
        if self.cursor.is_none() {
            self.anchor = None;
        }
    }

    fn clamp_editing(&mut self, rows: usize, columns: usize) {
        if let Some(editing) = &self.editing
            && (editing.cell.row >= rows || editing.cell.column >= columns)
        {
            self.editing = None;
        }
    }

    fn show_notices(&self, ui: &mut Ui, localize: &dyn Localize) {
        if let Some(notice) = self.notice {
            ui.label(notice.text(localize));
        }
        if self.refused_count > 0 {
            let count = i64::try_from(self.refused_count).unwrap_or(i64::MAX);
            let text = resolve(
                localize,
                messages::TABLE_PASTE_REFUSED.message_id,
                &[("count", Value::Int(count))],
            );
            ui.label(text);
        }
    }

    /// The columns drawn in the scrolling part, with a margin, never frozen ones.
    fn scroll_columns(&self, g: &Geometry) -> Range<usize> {
        let from = g.frozen_width + self.scroll_x;
        let span = visible_columns(&g.offsets, from, from + g.scroll_width);
        let first = span
            .start
            .max(g.frozen)
            .saturating_sub(COLUMN_MARGIN)
            .max(g.frozen);
        let end = span.end.saturating_add(COLUMN_MARGIN).min(g.columns);
        first..end.max(first)
    }

    /// The left edge of the column at visual position `index` on screen. Logical order runs left to right, or right to
    /// left in a right-to-left layout; the frozen columns never scroll.
    fn column_left(&self, g: &Geometry, index: usize) -> f32 {
        let shift = if index < g.frozen { 0.0 } else { self.scroll_x };
        if g.rtl {
            let end = g.offsets.get(index + 1).copied().unwrap_or(0.0);
            g.body.right() - end + shift
        } else {
            let at = g.offsets.get(index).copied().unwrap_or(0.0);
            g.body.left() + at - shift
        }
    }

    /// The horizontal extent the cells of the column at visual position `index` may be drawn in:
    /// the frozen columns at the logical start, the others beside them.
    fn region(&self, g: &Geometry, index: usize) -> (f32, f32) {
        let frozen = index < g.frozen;
        let fw = g.frozen_width;
        match (frozen, g.rtl) {
            (true, false) => (g.body.left(), g.body.left() + fw),
            (false, false) => (g.body.left() + fw, g.body.right()),
            (true, true) => (g.body.right() - fw, g.body.right()),
            (false, true) => (g.body.left(), g.body.right() - fw),
        }
    }

    /// `cell` cut to the column's region and to the vertical `band`. The result may be empty.
    fn clip_to_region(&self, g: &Geometry, index: usize, cell: Rect, band: Rect) -> Rect {
        let (lo, hi) = self.region(g, index);
        Rect::from_min_max(
            pos2(cell.left().max(lo), cell.top().max(band.top())),
            pos2(cell.right().min(hi), cell.bottom().min(band.bottom())),
        )
    }

    /// The top of row `row` on screen, computed from the first visible row so that the
    /// difference is a small integer.
    fn row_top(&self, g: &Geometry, row: usize) -> f32 {
        let steps = row.cast_signed() - self.scroll_row.cast_signed();
        g.body.top() + rows_to_points(steps, g.row_height) - self.scroll_offset
    }

    /// The rows whose cells are fetched and drawn: the rows in view plus [`MARGIN`] on each side.
    fn window(&self, g: &Geometry) -> Range<usize> {
        let in_view = f64_to_usize(((self.scroll_offset + g.body_height()) / g.row_height).ceil());
        let first = self.scroll_row.saturating_sub(MARGIN);
        let end = self
            .scroll_row
            .saturating_add(in_view)
            .saturating_add(MARGIN)
            .min(g.rows);
        first..end.max(first)
    }

    fn header(
        &mut self,
        ui: &mut Ui,
        source: &dyn TableSource,
        commands: &TableCommands,
        g: &Geometry,
        id: Id,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let mut intent = None;
        let positions = (0..g.frozen).chain(self.scroll_columns(g));
        for at in positions {
            let Some(index) = self.logical(at) else {
                continue;
            };
            let left = self.column_left(g, at);
            let width = self.width_at(at);
            let cell = Rect::from_min_size(pos2(left, g.header.top()), vec2(width, g.row_height));
            let visible = self.clip_to_region(g, at, cell, g.header);
            if !visible.is_positive() {
                continue;
            }
            let info = source.column(index).unwrap_or(ColumnInfo {
                name: String::new(),
                width,
                sortable: false,
            });
            let key = self
                .sort
                .iter()
                .enumerate()
                .find(|(_, key)| key.column == index)
                .map(|(rank, key)| (rank, *key));
            let (name, shown) = self.header_texts(localize, &info.name, key);
            let response =
                ui.interact(visible, id.with(("header", index)), Sense::click_and_drag());
            response.widget_info(|| WidgetInfo::labeled(Role::ColumnHeader, true, name.as_str()));
            if response.clicked() && info.sortable {
                let additive = ui.input(|i| i.modifiers.command);
                intent = self
                    .sort_click(source, commands, index, additive)
                    .or(intent);
            }
            self.reorder_by_drag(ui, g, index, &response);

            let painter = ui.painter().with_clip_rect(visible);
            painter.rect_filled(cell, 0.0, ui.visuals().widgets.inactive.weak_bg_fill);
            painter.text(
                text_point(cell, g.rtl, cell.center().y),
                self.text_align(g),
                shown,
                TextStyle::Body.resolve(ui.style()),
                ui.visuals().strong_text_color(),
            );
            self.resize_grip(ui, g, at, index, cell, id);
        }
        self.draw_drop_line(ui, g);
        intent
    }

    /// The accessible name and the shown text of a header: the name, then the sort direction
    /// arrow and, with several keys, the key's position.
    fn header_texts(
        &self,
        localize: &dyn Localize,
        name: &str,
        key: Option<(usize, Sort)>,
    ) -> (String, String) {
        let Some((rank, key)) = key else {
            return (name.to_owned(), name.to_owned());
        };
        let several = self.sort.len() > 1;
        let (direction, arrow) = match key.direction {
            SortDirection::Ascending => (messages::TABLE_ASCENDING, "▲"),
            SortDirection::Descending => (messages::TABLE_DESCENDING, "▼"),
        };
        let direction = Value::Text(direction.text(localize));
        let spoken = if several {
            resolve(
                localize,
                messages::TABLE_SORTED_KEY.message_id,
                &[
                    ("name", Value::Text(name.to_owned())),
                    ("direction", direction),
                    ("key", count_value(rank + 1)),
                    ("keys", count_value(self.sort.len())),
                ],
            )
        } else {
            resolve(
                localize,
                messages::TABLE_SORTED.message_id,
                &[
                    ("name", Value::Text(name.to_owned())),
                    ("direction", direction),
                ],
            )
        };
        let shown = if several {
            format!("{name} {arrow} {}", rank + 1)
        } else {
            format!("{name} {arrow}")
        };
        (spoken, shown)
    }

    /// Starts a reorder when a header drag starts, and moves the column where a drag ends.
    fn reorder_by_drag(&mut self, ui: &Ui, g: &Geometry, index: usize, response: &egui::Response) {
        if response.drag_started() {
            self.reorder = Some(index);
        }
        if response.drag_stopped() && self.reorder == Some(index) {
            self.reorder = None;
            if let Some(pointer) = ui.input(|i| i.pointer.latest_pos()) {
                self.drop_column(g, index, pointer.x);
            }
        }
    }

    /// Draws the column width grip at the logical end of a header cell and resizes the column
    /// while it is dragged.
    fn resize_grip(
        &mut self,
        ui: &mut Ui,
        g: &Geometry,
        at: usize,
        index: usize,
        cell: Rect,
        id: Id,
    ) {
        // The grip sits at the logical end of the column: the right edge, or the left edge in a
        // right-to-left layout.
        let end_x = if g.rtl { cell.left() } else { cell.right() };
        let grip = Rect::from_min_max(
            pos2(end_x - GRIP_WIDTH / 2.0, g.header.top()),
            pos2(end_x + GRIP_WIDTH / 2.0, g.header.bottom()),
        );
        let grip = self.clip_to_region(g, at, grip, g.header);
        if !grip.is_positive() {
            return;
        }
        let handle = ui.interact(grip, id.with(("grip", index)), Sense::DRAG);
        if handle.dragged() {
            let sign = if g.rtl { -1.0 } else { 1.0 };
            if let Some(slot) = self.widths.get_mut(index) {
                *slot = clamp_width(*slot + sign * handle.drag_delta().x);
            }
        }
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    }

    /// Draws the line where a dragged header would drop, and ends the reorder once the pointer
    /// is released.
    fn draw_drop_line(&mut self, ui: &Ui, g: &Geometry) {
        if let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
            && self.reorder.is_some()
        {
            let stroke = ui.visuals().selection.stroke;
            let from = pos2(pointer.x, g.header.top());
            ui.painter()
                .line_segment([from, pos2(pointer.x, g.body.bottom())], stroke);
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            self.reorder = None;
        }
    }

    /// The alignment of cell text: from the logical start.
    fn text_align(&self, g: &Geometry) -> egui::Align2 {
        if g.rtl {
            Direction::RightToLeft.start_align()
        } else {
            Direction::LeftToRight.start_align()
        }
    }

    fn body(&mut self, ui: &mut Ui, source: &dyn TableSource, g: &Geometry, id: Id) {
        let window = self.window(g);
        let scroll = self.scroll_columns(g);
        let frozen_columns: Vec<usize> = (0..g.frozen).filter_map(|at| self.logical(at)).collect();
        let scroll_columns: Vec<usize> = scroll.clone().filter_map(|at| self.logical(at)).collect();
        let frozen_cells = if !frozen_columns.is_empty() && !window.is_empty() {
            fetch(source, window.clone(), &frozen_columns)
        } else {
            Vec::new()
        };
        let scroll_cells = if !scroll_columns.is_empty() && !window.is_empty() {
            fetch(source, window.clone(), &scroll_columns)
        } else {
            Vec::new()
        };
        let font: FontId = TextStyle::Body.resolve(ui.style());
        let selection = self.selection();
        let cursor = self.cursor;
        let visuals = ui.visuals().clone();
        let mut clicked = None;
        let mut double = None;
        for row in window.clone() {
            let line = row - window.start;
            let y = self.row_top(g, row);
            if y + g.row_height < g.body.top() || y > g.body.bottom() {
                continue;
            }
            for at in (0..g.frozen).chain(scroll.clone()) {
                let text = if at < g.frozen {
                    cell_text(&frozen_cells, line, at)
                } else {
                    cell_text(&scroll_cells, line, at - scroll.start)
                };
                let Some(column) = self.logical(at) else {
                    continue;
                };
                let left = self.column_left(g, at);
                let cell =
                    Rect::from_min_size(pos2(left, y), vec2(self.width_at(at), g.row_height));
                let visible = self.clip_to_region(g, at, cell, g.body);
                if !visible.is_positive() {
                    continue;
                }
                let target = CellRef { row, column };
                let response = ui.interact(visible, id.with(("cell", row, column)), Sense::CLICK);
                response.widget_info(|| WidgetInfo::labeled(Role::Cell, true, text.to_owned()));
                if response.clicked() {
                    clicked = Some(target);
                }
                if response.double_clicked() {
                    double = Some(target);
                }
                let painter = ui.painter().with_clip_rect(visible);
                if selection.as_ref().is_some_and(|s| s.contains(target)) {
                    painter.rect_filled(cell, 0.0, visuals.selection.bg_fill);
                }
                let shown: String = text.chars().take(MAX_CELL_CHARS).collect();
                painter.text(
                    text_point(cell, g.rtl, cell.center().y),
                    self.text_align(g),
                    shown,
                    font.clone(),
                    visuals.text_color(),
                );
                if cursor == Some(target) {
                    painter.rect_stroke(
                        cell.shrink(1.0),
                        0.0,
                        visuals.selection.stroke,
                        StrokeKind::Inside,
                    );
                }
            }
        }
        if let Some(cell) = clicked {
            let extend = ui.input(|i| i.modifiers.shift);
            self.click(cell, extend);
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        if let Some(cell) = double {
            self.start_edit(source, cell);
        }
    }

    /// Starts an in-place edit of `cell` when the view has an edit command and the source
    /// reports the cell editable. The text is read from the source.
    fn start_edit(&mut self, source: &dyn TableSource, cell: CellRef) {
        if self.edit.is_none() || !source.editable(cell.row, cell.column) {
            return;
        }
        let text = source
            .cells(
                cell.row..cell.row.saturating_add(1),
                cell.column..cell.column.saturating_add(1),
            )
            .first()
            .and_then(|row| row.first())
            .cloned()
            .unwrap_or_default();
        self.editing = Some(Editing {
            cell,
            text,
            focus: true,
        });
    }

    /// Draws the in-place editor over the cell being edited. Enter returns the edit intent and
    /// closes the editor, Escape and losing focus close it without an intent.
    fn editor(
        &mut self,
        ui: &mut Ui,
        g: &Geometry,
        id: Id,
        localize: &dyn Localize,
    ) -> Option<Intent> {
        let mut editing = self.editing.take()?;
        let editor_id = id.with("editor");
        let mut commit = false;
        if ui.memory(|m| m.has_focus(editor_id)) {
            let (enter, escape) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if escape {
                ui.ctx().memory_mut(|m| m.request_focus(id));
                return None;
            }
            commit = enter;
        }
        if commit {
            ui.ctx().memory_mut(|m| m.request_focus(id));
            let command = self.edit.as_ref()?;
            return Some(
                Intent::new(command.clone())
                    .with("row", editing.cell.row)
                    .with("column", editing.cell.column)
                    .with("value", editing.text),
            );
        }
        let cell = self.cell_rect(g, editing.cell);
        let at = self.visual(editing.cell.column).unwrap_or(0);
        let (lo, hi) = self.region(g, at);
        let area = Rect::from_min_max(
            pos2(cell.left().max(lo), cell.top().max(g.body.top())),
            pos2(cell.right().min(hi), cell.bottom().min(g.body.bottom())),
        );
        if area.is_positive() {
            let label = messages::TABLE_EDIT.text(localize);
            let response = ui.put(
                area,
                TextEdit::singleline(&mut editing.text)
                    .id(editor_id)
                    .desired_width(f32::INFINITY),
            );
            response.widget_info(|| WidgetInfo::labeled(Role::TextInput, true, label.as_str()));
            if editing.focus {
                response.request_focus();
                editing.focus = false;
            } else if !response.has_focus() {
                // Focus moved elsewhere: the edit is cancelled.
                return None;
            }
        }
        self.editing = Some(editing);
        None
    }

    /// The rectangle of `cell` in screen coordinates, including the part scrolled out of view.
    fn cell_rect(&self, g: &Geometry, cell: CellRef) -> Rect {
        let at = self.visual(cell.column).unwrap_or(0);
        let left = self.column_left(g, at);
        Rect::from_min_size(
            pos2(left, self.row_top(g, cell.row)),
            vec2(self.width_at(at), g.row_height),
        )
    }

    /// Draws the vertical scroll bar at the right edge and the horizontal one at the bottom, and
    /// moves the view when they are clicked or dragged.
    fn scrollbars(&mut self, ui: &mut Ui, g: &Geometry, id: Id, localize: &dyn Localize) {
        let track_color = ui.visuals().widgets.inactive.weak_bg_fill;
        let thumb_color = ui.visuals().widgets.inactive.bg_fill;
        let rect = g.rect;

        // Vertical: the track is the height of the body.
        let vtrack = Rect::from_min_max(
            pos2(g.body.right(), g.body.top()),
            pos2(rect.right(), g.body.bottom()),
        );
        let (max_row, _) = bottom(g.rows, g.body_height(), g.row_height);
        let response = ui.interact(vtrack, id.with("vscroll"), Sense::click_and_drag());
        let label = messages::TABLE_SCROLL_VERTICAL.text(localize);
        response.widget_info(|| WidgetInfo::labeled(Role::ScrollBar, true, label.as_str()));
        if (response.clicked() || response.dragged())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let ratio = fraction_of(pos.y, vtrack.top(), vtrack.height());
            self.scroll_row = ratio_to_index(ratio, max_row);
            self.scroll_offset = 0.0;
            self.settle(g);
        }
        ui.painter().rect_filled(vtrack, 0.0, track_color);
        if g.rows > g.rows_fit() {
            let shown = to_f32(g.rows_fit()) / to_f32(g.rows);
            let length = thumb_length(vtrack.height(), shown);
            let position = ratio_of(self.scroll_row, max_row);
            let top = vtrack.top() + position * (vtrack.height() - length);
            let thumb = Rect::from_min_size(pos2(vtrack.left(), top), vec2(vtrack.width(), length));
            ui.painter().rect_filled(thumb, 0.0, thumb_color);
        }

        // Horizontal: the track is the width of the body, and the thumb starts at the logical
        // start (the right edge in a right-to-left layout).
        let htrack = Rect::from_min_max(
            pos2(g.body.left(), g.body.bottom()),
            pos2(g.body.right(), rect.bottom()),
        );
        let max_x = self.max_scroll_x(g);
        let response = ui.interact(htrack, id.with("hscroll"), Sense::click_and_drag());
        let label = messages::TABLE_SCROLL_HORIZONTAL.text(localize);
        response.widget_info(|| WidgetInfo::labeled(Role::ScrollBar, true, label.as_str()));
        if (response.clicked() || response.dragged())
            && let Some(pos) = response.interact_pointer_pos()
            && max_x > 0.0
        {
            let ratio = fraction_of(pos.x, htrack.left(), htrack.width());
            let ratio = if g.rtl { 1.0 - ratio } else { ratio };
            self.scroll_x = clamp_offset(ratio * max_x, max_x);
        }
        ui.painter().rect_filled(htrack, 0.0, track_color);
        if max_x > 0.0 {
            let shown = g.scroll_width / g.scrollable_width().max(1.0);
            let length = thumb_length(htrack.width(), shown);
            let position = self.scroll_x / max_x;
            let position = if g.rtl { 1.0 - position } else { position };
            let left = htrack.left() + position * (htrack.width() - length);
            let thumb =
                Rect::from_min_size(pos2(left, htrack.top()), vec2(length, htrack.height()));
            ui.painter().rect_filled(thumb, 0.0, thumb_color);
        }
    }

    fn keyboard(
        &mut self,
        ui: &mut Ui,
        source: &dyn TableSource,
        commands: &TableCommands,
        g: &Geometry,
    ) -> Option<Intent> {
        let mut intent = None;
        if g.rows == 0 || g.columns == 0 {
            return None;
        }
        self.cursor.get_or_insert(CellRef::default());
        self.anchor.get_or_insert(CellRef::default());
        let mirror: isize = if g.rtl { -1 } else { 1 };

        // Alt with Shift and the horizontal arrows moves the cursor column along the order. It is
        // read before the resize below, which matches the same arrows without Shift.
        let (later, earlier) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowRight),
                i.consume_key(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowLeft),
            )
        });
        if let Some(cursor) = self.cursor
            && (later || earlier)
            && let Some(at) = self.visual(cursor.column)
        {
            let direction: isize = if later { 1 } else { -1 };
            let to = at
                .checked_add_signed(direction * mirror)
                .filter(|&to| to < g.columns);
            if let Some(to) = to {
                // Both positions are drawn columns, so the move is accepted.
                let _ = self.move_column(at, to);
            }
        }

        // Alt with the horizontal arrows resizes the cursor column.
        let (wider, narrower) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::ALT, Key::ArrowRight),
                i.consume_key(Modifiers::ALT, Key::ArrowLeft),
            )
        });
        if let Some(cursor) = self.cursor
            && (wider || narrower)
            && let Some(slot) = self.widths.get_mut(cursor.column)
        {
            let delta = if wider { RESIZE_STEP } else { -RESIZE_STEP };
            *slot = clamp_width(*slot + delta);
        }

        let page = to_steps(g.rows_fit() as f32);
        for (nav, extend) in ui.input_mut(take_navigation) {
            let Some(cursor) = self.cursor else { break };
            let target = match nav {
                Nav::Rows(d) => CellRef {
                    row: step(cursor.row, d, g.rows),
                    column: cursor.column,
                },
                Nav::Columns(d) => {
                    let at = self.visual(cursor.column).unwrap_or(0);
                    let to = step(at, d * mirror, g.columns);
                    CellRef {
                        row: cursor.row,
                        column: self.logical(to).unwrap_or(cursor.column),
                    }
                }
                Nav::Pages(d) => CellRef {
                    row: step(cursor.row, d.saturating_mul(page), g.rows),
                    column: cursor.column,
                },
                Nav::Home => CellRef {
                    row: 0,
                    column: cursor.column,
                },
                Nav::End => CellRef {
                    row: g.rows - 1,
                    column: cursor.column,
                },
            };
            self.click(target, extend);
            self.follow(g, target);
        }

        let f2 = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F2));
        if f2 && let Some(cursor) = self.cursor {
            self.start_edit(source, cursor);
        }

        // Ctrl or Cmd with Enter adds or toggles a secondary key; Enter alone edits or sorts.
        let additive = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        let enter = !additive && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        if (enter || additive)
            && let Some(cursor) = self.cursor
        {
            if enter && self.edit.is_some() && source.editable(cursor.row, cursor.column) {
                self.start_edit(source, cursor);
            } else if source.column(cursor.column).is_some_and(|c| c.sortable) {
                intent = self.sort_click(source, commands, cursor.column, additive);
            }
        }

        if take_events(ui, |e| matches!(e, Event::Copy))
            .next()
            .is_some()
        {
            self.copy(ui, source, g);
        }
        for event in take_events(ui, |e| matches!(e, Event::Paste(_))) {
            if let Event::Paste(text) = event {
                intent = self.paste(source, commands, g, &text).or(intent);
            }
        }
        intent
    }

    /// Scrolls the body (and the scrolling columns) so that `cell` is inside the viewport.
    fn follow(&mut self, g: &Geometry, cell: CellRef) {
        let shown = g.rows_fit();
        if cell.row <= self.scroll_row {
            self.scroll_row = cell.row;
            self.scroll_offset = 0.0;
        } else if cell.row >= self.scroll_row.saturating_add(shown) {
            self.scroll_row = cell.row + 1 - shown;
            self.scroll_offset = 0.0;
        }
        self.settle(g);
        let Some(at) = self.visual(cell.column) else {
            return;
        };
        if at >= g.frozen {
            let start = g.offsets.get(at).copied().unwrap_or(0.0) - g.frozen_width;
            let length = self.width_at(at);
            self.scroll_x = scroll_to_show(self.scroll_x, g.scroll_width, start, length);
        }
    }

    /// Copies the selected cells as text. Columns run in the drawn order from the logical start,
    /// whatever the layout direction; rows run top to bottom.
    fn copy(&mut self, ui: &Ui, source: &dyn TableSource, g: &Geometry) {
        let Some(selection) = self.selection() else {
            return;
        };
        let rows = selection.rows.start.min(g.rows)..selection.rows.end.min(g.rows);
        if rows.len().saturating_mul(selection.columns.len()) > MAX_COPY_CELLS {
            self.notice = Some(messages::TABLE_COPY_TOO_LARGE);
            return;
        }
        self.notice = None;
        let cells = fetch(source, rows, &selection.columns);
        ui.ctx().copy_text(format_grid(&cells));
    }

    /// Pastes the grid at the top-left corner of the selection. Each pasted column goes to the
    /// next drawn column, so a paste follows the order.
    fn paste(
        &mut self,
        source: &dyn TableSource,
        commands: &TableCommands,
        g: &Geometry,
        text: &str,
    ) -> Option<Intent> {
        let grid = match parse_grid(text) {
            Ok(grid) => grid,
            Err(error) => {
                self.notice = Some(Message {
                    message_id: error.message_id(),
                });
                return None;
            }
        };
        self.notice = None;
        let (origin_row, origin_at) = match self.selection() {
            Some(s) => (
                s.rows.start,
                s.columns.first().and_then(|&c| self.visual(c)).unwrap_or(0),
            ),
            None => (0, 0),
        };
        let mut accepted = Vec::new();
        let mut refused = Vec::new();
        let mut refused_count = 0usize;
        for (dr, line) in grid.iter().enumerate() {
            for (dc, value) in line.iter().enumerate() {
                let position = origin_at.saturating_add(dc);
                let cell = CellRef {
                    row: origin_row.saturating_add(dr),
                    column: self.logical(position).unwrap_or(position),
                };
                let in_table = cell.row < g.rows && position < g.columns;
                if in_table && source.accepts(cell.row, cell.column, value) {
                    accepted.push(serde_json::json!({
                        "row": cell.row,
                        "column": cell.column,
                        "value": value,
                    }));
                } else {
                    refused_count = refused_count.saturating_add(1);
                    if refused.len() < MAX_REPORTED_REFUSALS {
                        refused.push(cell);
                    }
                }
            }
        }
        self.refused = refused;
        self.refused_count = refused_count;
        (!accepted.is_empty()).then(|| {
            Intent::new(commands.paste.clone()).with("cells", serde_json::Value::Array(accepted))
        })
    }
}

/// The intent of a sort click: the key the click set (`column`, `direction`) and the table's keys
/// after it (`keys`, first key first).
fn sort_intent(commands: &TableCommands, clicked: Sort, keys: &[Sort]) -> Intent {
    let keys = keys
        .iter()
        .map(|key| {
            serde_json::json!({
                "column": key.column,
                "direction": key.direction.as_str(),
            })
        })
        .collect();
    Intent::new(commands.sort.clone())
        .with("column", clicked.column)
        .with("direction", clicked.direction.as_str())
        .with("keys", serde_json::Value::Array(keys))
}

/// A count as a message argument.
fn count_value(count: usize) -> Value {
    Value::Int(i64::try_from(count).unwrap_or(i64::MAX))
}

/// The texts of `rows` in `columns` (source column indices in the order given): one list per row,
/// one text per column. The source is asked once per run of consecutive indices, so a reordered
/// table asks for each run of columns and no others.
fn fetch(source: &dyn TableSource, rows: Range<usize>, columns: &[usize]) -> Vec<Vec<String>> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &column in columns {
        match runs.last_mut() {
            Some((first, count)) if first.saturating_add(*count) == column => *count += 1,
            _ => runs.push((column, 1)),
        }
    }
    let mut lines: Vec<Vec<String>> = vec![Vec::with_capacity(columns.len()); rows.len()];
    for (first, count) in runs {
        let texts = source.cells(rows.clone(), first..first.saturating_add(count));
        for (line, cells) in lines.iter_mut().enumerate() {
            let row = texts.get(line);
            cells.extend(
                (0..count).map(|k| row.and_then(|r| r.get(k)).cloned().unwrap_or_default()),
            );
        }
    }
    lines
}

/// The point where a cell's text starts: inside the left edge, or the right edge in a
/// right-to-left layout, at height `y`.
fn text_point(cell: Rect, rtl: bool, y: f32) -> Pos2 {
    if rtl {
        pos2(cell.right() - 6.0, y)
    } else {
        pos2(cell.left() + 6.0, y)
    }
}

/// Removes the navigation key presses from the input, in order, and returns them with whether
/// Shift is held. Presses with Alt, Ctrl or Command are left for others.
fn take_navigation(i: &mut egui::InputState) -> Vec<(Nav, bool)> {
    let mut taken = Vec::new();
    i.events.retain(|event| {
        let Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
        else {
            return true;
        };
        let nav = match key {
            Key::ArrowUp => Nav::Rows(-1),
            Key::ArrowDown => Nav::Rows(1),
            Key::ArrowLeft => Nav::Columns(-1),
            Key::ArrowRight => Nav::Columns(1),
            Key::PageUp => Nav::Pages(-1),
            Key::PageDown => Nav::Pages(1),
            Key::Home => Nav::Home,
            Key::End => Nav::End,
            _ => return true,
        };
        if modifiers.alt || modifiers.ctrl || modifiers.command {
            return true;
        }
        taken.push((nav, modifiers.shift));
        false
    });
    taken
}

/// Removes the events matching `wanted` from the input and returns them.
fn take_events(ui: &Ui, wanted: impl Fn(&Event) -> bool) -> std::vec::IntoIter<Event> {
    let taken: Vec<Event> = ui.input_mut(|i| {
        let mut taken = Vec::new();
        i.events.retain(|e| {
            if wanted(e) {
                taken.push(e.clone());
                false
            } else {
                true
            }
        });
        taken
    });
    taken.into_iter()
}

fn cell_text(data: &[Vec<String>], line: usize, column: usize) -> &str {
    data.get(line)
        .and_then(|row| row.get(column))
        .map_or("", String::as_str)
}

fn step(at: usize, delta: isize, len: usize) -> usize {
    let last = len.saturating_sub(1);
    let moved = if delta < 0 {
        at.saturating_sub(delta.unsigned_abs())
    } else {
        at.saturating_add(delta.unsigned_abs())
    };
    moved.min(last)
}

/// The number of rows one page moves: at least one.
fn to_steps(rows_in_page: f32) -> isize {
    isize::try_from(f64_to_usize(rows_in_page).max(1)).unwrap_or(isize::MAX)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturating conversion of a finite, non-negative size"
)]
fn f64_to_usize(value: f32) -> usize {
    value as usize
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display coordinates; the value is a count of rows or columns, exact below 2^24"
)]
fn to_f32(n: usize) -> f32 {
    n as f32
}

/// Whole steps of a finite float, saturating.
#[allow(
    clippy::cast_possible_truncation,
    reason = "saturating conversion of a step count, clamped first"
)]
fn steps_of(value: f32) -> isize {
    if value.is_finite() {
        value.clamp(-1.0e12, 1.0e12) as isize
    } else {
        0
    }
}

/// The vertical distance of `steps` rows, exact for a small count of rows.
#[allow(
    clippy::cast_precision_loss,
    reason = "a count of rows next to the first visible one; exact below 2^24"
)]
fn rows_to_points(steps: isize, row_height: f32) -> f32 {
    steps as f32 * row_height
}

/// The fraction `[0, 1]` of `position` along a track that starts at `start` and is `length` long.
fn fraction_of(position: f32, start: f32, length: f32) -> f32 {
    if length > 0.0 && position.is_finite() {
        ((position - start) / length).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The row index at `ratio` of the way through `max` rows.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to the range of rows"
)]
fn ratio_to_index(ratio: f32, max: usize) -> usize {
    (f64::from(ratio) * max as f64)
        .round()
        .clamp(0.0, max as f64) as usize
}

/// The position `index` of `max` as a fraction.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a fraction in [0, 1] of a scroll bar's track; the narrowing is intended"
)]
fn ratio_of(index: usize, max: usize) -> f32 {
    if max == 0 {
        0.0
    } else {
        (index as f64 / max as f64).min(1.0) as f32
    }
}

/// The length of a scroll thumb for a visible `share` of the track of `track` points.
fn thumb_length(track: f32, share: f32) -> f32 {
    (track * share.clamp(0.0, 1.0)).clamp((BAR_WIDTH * 2.0).min(track), track.max(0.0))
}

fn span(a: usize, b: usize) -> Range<usize> {
    a.min(b)..a.max(b).saturating_add(1)
}

fn clamp_cell(cell: CellRef, rows: usize, columns: usize) -> Option<CellRef> {
    (rows > 0 && columns > 0).then(|| CellRef {
        row: cell.row.min(rows - 1),
        column: cell.column.min(columns - 1),
    })
}

fn clamp_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(MIN_WIDTH, MAX_WIDTH)
    } else {
        DEFAULT_WIDTH
    }
}

fn clamp_offset(offset: f32, max: f32) -> f32 {
    if offset.is_finite() {
        offset.clamp(0.0, max.max(0.0))
    } else {
        0.0
    }
}

/// The greatest scroll position of a table of `rows` rows in a body of `body` points, as a row
/// index and an offset below it. Computed from whole rows, so it is exact for any row count.
#[must_use]
pub fn bottom(rows: usize, body: f32, row_height: f32) -> (usize, f32) {
    let fit = f64_to_usize((body / row_height).floor());
    if fit == 0 {
        return (rows.saturating_sub(1), 0.0);
    }
    if rows <= fit {
        return (0, 0.0);
    }
    let rest = body - to_f32(fit) * row_height;
    if rest > 0.0 {
        (rows - fit - 1, row_height - rest)
    } else {
        (rows - fit, 0.0)
    }
}

/// The left edges of the columns of `widths`, then the right edge of the last one.
#[must_use]
pub fn column_offsets(widths: &[f32]) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(widths.len() + 1);
    let mut at = 0.0_f32;
    offsets.push(at);
    for width in widths {
        at += width;
        offsets.push(at);
    }
    offsets
}

/// The rows whose extent meets the span from `offset` to `offset + height`, for rows of
/// `row_height` points, clamped to `rows`.
#[must_use]
pub fn visible_rows(offset: f32, height: f32, row_height: f32, rows: usize) -> Range<usize> {
    if rows == 0 || row_height.is_nan() || row_height <= 0.0 {
        return 0..0;
    }
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let first = index_at(offset, row_height).min(rows);
    let last = end_index(offset + height.max(0.0), row_height).min(rows);
    first..last.max(first)
}

/// The columns whose span (`offsets[c]` to `offsets[c + 1]`) meets `from` to `to`.
#[must_use]
pub fn visible_columns(offsets: &[f32], from: f32, to: f32) -> Range<usize> {
    let count = offsets.len().saturating_sub(1);
    let first = offsets
        .partition_point(|&at| at <= from)
        .saturating_sub(1)
        .min(count);
    let end = offsets.partition_point(|&at| at < to).min(count);
    first..end.max(first)
}

/// The index of the row that starts after `position`: the exclusive end of the rows above it.
fn end_index(position: f32, size: f32) -> usize {
    let steps = (position / size).ceil();
    if steps.is_finite() && steps > 0.0 {
        f64_to_usize(steps)
    } else {
        0
    }
}

fn index_at(position: f32, size: f32) -> usize {
    let steps = (position / size).floor();
    if steps.is_finite() && steps > 0.0 {
        f64_to_usize(steps)
    } else {
        0
    }
}

/// The offset that shows the span from `start` for `length` points in a viewport of `viewport`
/// points that begins at `offset`: unchanged when the span is already inside.
#[must_use]
pub fn scroll_to_show(offset: f32, viewport: f32, start: f32, length: f32) -> f32 {
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let end = start + length;
    if start < offset {
        start.max(0.0)
    } else if end > offset + viewport {
        (end - viewport).max(0.0)
    } else {
        offset
    }
}

/// Formats a grid as tab-separated text. Each record ends with a line feed. A cell that contains
/// a tab, a line break or a quote is wrapped in quotes, with each quote doubled. A grid row must
/// have at least one cell: an empty row reads back as one empty cell.
#[must_use]
pub fn format_grid(rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                out.push('\t');
            }
            if cell.contains(['\t', '\n', '\r', '"']) {
                out.push('"');
                for c in cell.chars() {
                    if c == '"' {
                        out.push('"');
                    }
                    out.push(c);
                }
                out.push('"');
            } else {
                out.push_str(cell);
            }
        }
        out.push('\n');
    }
    out
}

/// Reads tab-separated text into a grid, the inverse of [`format_grid`]. A record ends at a line
/// feed, a carriage return or a carriage return and line feed; a final record needs no line
/// feed. Empty text has no records. A quoted cell may contain tabs, line breaks and doubled
/// quotes; characters after a closing quote are kept with the cell.
///
/// # Errors
///
/// `ui.table.too_large` when the text has more than [`MAX_PASTE_BYTES`] bytes or more than
/// [`MAX_PASTE_CELLS`] cells.
pub fn parse_grid(text: &str) -> Result<Vec<Vec<String>>> {
    if text.len() > MAX_PASTE_BYTES {
        return Err(Error::new(codes::TABLE_TOO_LARGE));
    }
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut chars = text.chars().peekable();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cells = 0usize;
    loop {
        let mut field = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c != '"' {
                    field.push(c);
                } else if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    break;
                }
            }
            while let Some(&c) = chars.peek() {
                if matches!(c, '\t' | '\n' | '\r') {
                    break;
                }
                field.push(c);
                chars.next();
            }
        } else {
            while let Some(&c) = chars.peek() {
                if matches!(c, '\t' | '\n' | '\r') {
                    break;
                }
                field.push(c);
                chars.next();
            }
        }
        row.push(field);
        cells += 1;
        if cells > MAX_PASTE_CELLS {
            return Err(Error::new(codes::TABLE_TOO_LARGE));
        }
        match chars.next() {
            Some('\t') => {}
            Some(end) => {
                if end == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.push(std::mem::take(&mut row));
                if chars.peek().is_none() {
                    break;
                }
            }
            None => {
                rows.push(std::mem::take(&mut row));
                break;
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn visible_rows_are_the_rows_meeting_the_viewport() {
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 1_000_000), 0..5);
        assert_eq!(visible_rows(30.0, 100.0, 20.0, 1_000_000), 1..7);
        assert_eq!(
            visible_rows(999_990.0 * 20.0, 100.0, 20.0, 1_000_000),
            999_990..999_995
        );
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 3), 0..3);
        assert_eq!(visible_rows(500.0, 100.0, 20.0, 3), 3..3);
        assert_eq!(visible_rows(0.0, 100.0, 0.0, 3), 0..0);
        assert_eq!(visible_rows(f32::NAN, 100.0, 20.0, 3), 0..3);
    }

    #[test]
    fn visible_columns_find_the_span_under_the_viewport() {
        let offsets = column_offsets(&[100.0, 100.0, 100.0]);
        assert_eq!(visible_columns(&offsets, 0.0, 150.0), 0..2);
        assert_eq!(visible_columns(&offsets, 130.0, 250.0), 1..3);
        assert_eq!(visible_columns(&offsets, 400.0, 500.0), 3..3);
        assert_eq!(visible_columns(&[0.0], 0.0, 10.0), 0..0);
    }

    #[test]
    fn scrolling_keeps_a_visible_span_and_brings_a_hidden_one_in() {
        assert_eq!(scroll_to_show(0.0, 100.0, 40.0, 20.0), 0.0);
        assert_eq!(scroll_to_show(0.0, 100.0, 120.0, 20.0), 40.0);
        assert_eq!(scroll_to_show(200.0, 100.0, 120.0, 20.0), 120.0);
        assert_eq!(scroll_to_show(f32::NAN, 100.0, 0.0, 20.0), 0.0);
    }

    #[test]
    fn the_bottom_of_a_very_long_table_is_exact() {
        // 50,000,000 rows of 20 points in a 100 point body: four whole rows and a rest of 20
        // points, so the bottom is row 49,999,995 scrolled by nothing.
        assert_eq!(bottom(50_000_000, 100.0, 20.0), (49_999_995, 0.0));
        // A body of 110 points leaves a rest of 10 points: the first row is scrolled up by 10.
        assert_eq!(bottom(50_000_000, 110.0, 20.0), (49_999_994, 10.0));
        assert_eq!(bottom(3, 100.0, 20.0), (0, 0.0));
        assert_eq!(bottom(0, 100.0, 20.0), (0, 0.0));
    }

    #[test]
    fn hostile_clipboard_text_is_read_without_panic() {
        assert_eq!(
            parse_grid("\"unterminated").unwrap(),
            vec![vec!["unterminated".to_owned()]]
        );
        assert_eq!(
            parse_grid("\"a\"b\tc").unwrap(),
            vec![vec!["ab".to_owned(), "c".to_owned()]]
        );
        assert!(parse_grid(&"x".repeat(MAX_PASTE_BYTES + 1)).is_err());
        assert_eq!(parse_grid("").unwrap(), Vec::<Vec<String>>::new());
        assert_eq!(parse_grid("\n").unwrap(), vec![vec![String::new()]]);
    }

    fn cell() -> impl Strategy<Value = String> {
        "[a-c \t\n\r\"]{0,4}|\\PC{0,6}"
    }

    proptest! {
        #[test]
        fn format_then_parse_returns_the_same_grid(
            grid in proptest::collection::vec(proptest::collection::vec(cell(), 1..5), 0..6),
        ) {
            let text = format_grid(&grid);
            prop_assert_eq!(parse_grid(&text).unwrap(), grid);
        }

        #[test]
        fn parse_never_panics(text in "\\PC{0,200}") {
            let _ = parse_grid(&text);
        }
    }
}
