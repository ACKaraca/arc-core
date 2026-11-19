//! The virtualized table (CORE-UI-013) and the table clipboard (CORE-UI-014).
//!
//! A [`TableView`] draws a [`TableSource`]. The source answers row and column counts, column
//! descriptions and cell texts for a window of rows and a span of columns; the view asks only
//! for the rows inside the viewport plus [`MARGIN`] rows on each side, so a frame costs the same
//! for a million rows as for a hundred. Leading columns named by [`TableView::frozen`] stay at
//! the left edge while the others scroll horizontally.
//!
//! The view never sorts: a click on a sortable header, or Enter on the cursor column, returns the
//! intent of [`TableCommands::sort`] with `column` and `direction`, and the application reports
//! it to the source. Cell selection, column widths and scroll positions are view state. A paste
//! of tab-separated text returns one intent of [`TableCommands::paste`] with the accepted cells
//! (`cells`, each with `row`, `column` and `value`); the source decides per cell through
//! [`TableSource::accepts`], and refused cells are kept in [`TableView::refused`].
//!
//! Copy and paste use the events egui delivers to a focused widget: `Event::Copy` puts the
//! selected cells on the clipboard through [`egui::Context::copy_text`], and `Event::Paste` carries
//! the text that [`parse_grid`] reads. [`format_grid`] and [`parse_grid`] are pure and inverse to
//! each other for grids without empty rows.

use std::ops::Range;

use arc_base::error::Value;
use arc_base::{Error, Result};
use egui::{
    Align2, CursorIcon, Event, Id, Key, Modifiers, Rect, Role, ScrollArea, Sense, StrokeKind,
    TextStyle, Ui, UiBuilder, WidgetInfo, pos2, vec2,
};

use crate::command::Intent;
use crate::error::codes;
use crate::text::{Localize, Message, messages, resolve};

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
    /// margin, and for the columns in view and the frozen ones.
    fn cells(&self, rows: Range<usize>, columns: Range<usize>) -> Vec<Vec<String>>;
    /// The sort the source currently applies, shown in the header.
    fn sorted(&self) -> Option<Sort>;
    /// Whether cell (`row`, `column`) accepts `text` from a paste.
    fn accepts(&self, row: usize, column: usize, text: &str) -> bool;
}

/// The intent names of a table: `sort` takes `column` and `direction`; `paste` takes `cells`, an
/// array of objects with `row`, `column` and `value`.
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

/// The selected rectangle, as half-open ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The selected rows.
    pub rows: Range<usize>,
    /// The selected columns.
    pub columns: Range<usize>,
}

impl Selection {
    /// Whether `cell` is inside the selection.
    #[must_use]
    pub fn contains(&self, cell: CellRef) -> bool {
        self.rows.contains(&cell.row) && self.columns.contains(&cell.column)
    }
}

/// The table's view state: frozen columns, column widths, scroll positions, the selection and
/// the report of the last paste.
#[derive(Debug, Clone, Default)]
pub struct TableView {
    /// Leading columns that stay at the left edge while the others scroll.
    pub frozen: usize,
    widths: Vec<f32>,
    scroll_x: f32,
    scroll_y: f32,
    anchor: Option<CellRef>,
    cursor: Option<CellRef>,
    refused: Vec<CellRef>,
    refused_count: usize,
    notice: Option<Message>,
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
    rect: Rect,
    body: Rect,
    rows: usize,
    columns: usize,
    frozen: usize,
    /// Left edge of each column in the table's own coordinates, plus the right edge of the last.
    offsets: Vec<f32>,
    row_height: f32,
    frozen_width: f32,
    scroll_width: f32,
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
}

impl TableView {
    /// The selected rectangle, or `None` when no cell is selected.
    #[must_use]
    pub fn selection(&self) -> Option<Selection> {
        let (anchor, cursor) = (self.anchor?, self.cursor?);
        Some(Selection {
            rows: span(anchor.row, cursor.row),
            columns: span(anchor.column, cursor.column),
        })
    }

    /// The cell with the keyboard focus.
    #[must_use]
    pub fn cursor(&self) -> Option<CellRef> {
        self.cursor
    }

    /// The width of column `index`, once the view has drawn the table.
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

    fn sync_widths(&mut self, source: &dyn TableSource, columns: usize) {
        for index in self.widths.len()..columns {
            let width = source.column(index).map_or(DEFAULT_WIDTH, |c| c.width);
            self.widths.push(clamp_width(width));
        }
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

    /// Draws the table in the space left in `ui` and returns the intent of a sort or a paste.
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
        self.clamp_selection(rows, columns);
        self.show_notices(ui, localize);

        let row_height = ui.spacing().interact_size.y.max(1.0);
        let size = vec2(
            ui.available_width().max(1.0),
            ui.available_height().max(row_height * 2.0),
        );
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let id = ui.scope_id().with("arc-table");
        let container = ui.interact(rect, id, Sense::click());
        let label = messages::TABLE.text(localize);
        container.widget_info(|| WidgetInfo::labeled(Role::Table, true, label.as_str()));
        if container.clicked() {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }

        let mut g = self.geometry(rect, rows, columns, row_height);
        let mut intent = None;
        if container.has_focus() {
            intent = self.keyboard(ui, source, commands, &g);
        }
        let max_x = (g.scrollable_width() - g.scroll_width).max(0.0);
        self.scroll_x = clamp_offset(self.scroll_x, max_x);
        g.body = Rect::from_min_max(pos2(rect.left(), rect.top() + row_height), rect.max);

        intent = intent.or(self.header(ui, source, commands, &g, id, localize));
        let font = TextStyle::Body.resolve(ui.style());
        let offset = self.scroll_y;
        let area = ui.scope_builder(UiBuilder::new().max_rect(g.body), |ui| {
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .vertical_scroll_offset(offset)
                .show_viewport(ui, |ui, viewport| {
                    self.body(ui, source, &g, viewport, id, &font);
                })
        });
        self.scroll_y = area.inner.state.clamped_offset().y;
        intent
    }

    fn geometry(&self, rect: Rect, rows: usize, columns: usize, row_height: f32) -> Geometry {
        let offsets = column_offsets(
            &self
                .widths
                .iter()
                .take(columns)
                .copied()
                .collect::<Vec<f32>>(),
        );
        let frozen = self.frozen.min(columns);
        let frozen_width = offsets.get(frozen).copied().unwrap_or(0.0);
        let total = offsets.last().copied().unwrap_or(0.0);
        let scroll_width = (rect.width() - frozen_width).max(0.0);
        Geometry {
            rect,
            body: Rect::from_min_max(pos2(rect.left(), rect.top() + row_height), rect.max),
            rows,
            columns,
            frozen,
            offsets,
            row_height,
            frozen_width,
            scroll_width: scroll_width.min(total),
        }
    }

    fn clamp_selection(&mut self, rows: usize, columns: usize) {
        self.anchor = self.anchor.and_then(|c| clamp_cell(c, rows, columns));
        self.cursor = self.cursor.and_then(|c| clamp_cell(c, rows, columns));
        if self.cursor.is_none() {
            self.anchor = None;
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

    /// The left edge of column `index` on screen, after the horizontal scroll.
    fn column_left(&self, g: &Geometry, index: usize) -> f32 {
        let at = g.offsets.get(index).copied().unwrap_or(0.0);
        let shift = if index < g.frozen { 0.0 } else { self.scroll_x };
        g.rect.left() + at - shift
    }

    /// The region a column's cells may be drawn in: frozen columns at the left, the others to
    /// the right of them.
    fn region(&self, g: &Geometry, index: usize) -> Rect {
        if index < g.frozen {
            Rect::from_min_max(
                g.rect.min,
                pos2(g.rect.left() + g.frozen_width, g.rect.bottom()),
            )
        } else {
            Rect::from_min_max(
                pos2(g.rect.left() + g.frozen_width, g.rect.top()),
                g.rect.max,
            )
        }
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
        let header_top = g.rect.top();
        let sort = source.sorted();
        let columns = (0..g.frozen).chain(self.scroll_columns(g));
        for index in columns {
            let left = self.column_left(g, index);
            let width = self.widths.get(index).copied().unwrap_or(DEFAULT_WIDTH);
            let cell = Rect::from_min_size(pos2(left, header_top), vec2(width, g.row_height));
            let region = self.region(g, index);
            let visible = cell.intersect(region);
            if !visible.is_positive() {
                continue;
            }
            let info = source.column(index).unwrap_or(ColumnInfo {
                name: String::new(),
                width,
                sortable: false,
            });
            let sorted = sort.filter(|s| s.column == index);
            let name = match sorted {
                Some(s) => {
                    let direction = match s.direction {
                        SortDirection::Ascending => messages::TABLE_ASCENDING,
                        SortDirection::Descending => messages::TABLE_DESCENDING,
                    };
                    resolve(
                        localize,
                        messages::TABLE_SORTED.message_id,
                        &[
                            ("name", Value::Text(info.name.clone())),
                            ("direction", Value::Text(direction.text(localize))),
                        ],
                    )
                }
                None => info.name.clone(),
            };
            let shown = match sorted {
                Some(s) if s.direction == SortDirection::Ascending => format!("{} ▲", info.name),
                Some(_) => format!("{} ▼", info.name),
                None => info.name.clone(),
            };
            let response = ui.interact(visible, id.with(("header", index)), Sense::CLICK);
            response.widget_info(|| WidgetInfo::labeled(Role::ColumnHeader, true, name.as_str()));
            if response.clicked() && info.sortable {
                intent = Some(sort_intent(commands, index, sort));
            }
            let painter = ui.painter().with_clip_rect(visible);
            painter.rect_filled(cell, 0.0, ui.visuals().widgets.inactive.weak_bg_fill);
            painter.text(
                pos2(cell.left() + 6.0, cell.center().y),
                Align2::LEFT_CENTER,
                shown,
                TextStyle::Body.resolve(ui.style()),
                ui.visuals().strong_text_color(),
            );

            let grip = Rect::from_min_max(
                pos2(cell.right() - GRIP_WIDTH / 2.0, header_top),
                pos2(cell.right() + GRIP_WIDTH / 2.0, g.rect.top() + g.row_height),
            )
            .intersect(region);
            if grip.is_positive() {
                let handle = ui.interact(grip, id.with(("grip", index)), Sense::DRAG);
                if handle.dragged() {
                    let width = self.widths.get(index).copied().unwrap_or(DEFAULT_WIDTH);
                    if let Some(slot) = self.widths.get_mut(index) {
                        *slot = clamp_width(width + handle.drag_delta().x);
                    }
                }
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
            }
        }
        intent
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        source: &dyn TableSource,
        g: &Geometry,
        viewport: Rect,
        id: Id,
        font: &egui::FontId,
    ) {
        ui.set_height(to_f32(g.rows) * g.row_height);
        let top = ui.max_rect().top();
        let clip = ui.clip_rect();
        let window = padded(
            visible_rows(viewport.min.y, viewport.height(), g.row_height, g.rows),
            MARGIN,
            g.rows,
        );
        let frozen_cells = if g.frozen > 0 && !window.is_empty() {
            source.cells(window.clone(), 0..g.frozen)
        } else {
            Vec::new()
        };
        let scroll = self.scroll_columns(g);
        let scroll_cells = if !scroll.is_empty() && !window.is_empty() {
            source.cells(window.clone(), scroll.clone())
        } else {
            Vec::new()
        };
        let selection = self.selection();
        let cursor = self.cursor;
        let visuals = ui.visuals().clone();
        let mut clicked = None;
        for (line, row) in window.clone().enumerate() {
            let y = top + to_f32(row) * g.row_height;
            if y + g.row_height < clip.top() || y > clip.bottom() {
                continue;
            }
            for column in (0..g.frozen).chain(scroll.clone()) {
                let text = if column < g.frozen {
                    cell_text(&frozen_cells, line, column)
                } else {
                    cell_text(&scroll_cells, line, column - scroll.start)
                };
                let left = self.column_left(g, column);
                let width = self.widths.get(column).copied().unwrap_or(DEFAULT_WIDTH);
                let cell = Rect::from_min_size(pos2(left, y), vec2(width, g.row_height));
                let visible = cell.intersect(self.region(g, column)).intersect(clip);
                if !visible.is_positive() {
                    continue;
                }
                let at = CellRef { row, column };
                let response = ui.interact(visible, id.with(("cell", row, column)), Sense::CLICK);
                response.widget_info(|| WidgetInfo::labeled(Role::Cell, true, text.to_owned()));
                if response.clicked() {
                    clicked = Some(at);
                }
                let painter = ui.painter().with_clip_rect(visible);
                if selection.as_ref().is_some_and(|s| s.contains(at)) {
                    painter.rect_filled(cell, 0.0, visuals.selection.bg_fill);
                }
                let shown: String = text.chars().take(MAX_CELL_CHARS).collect();
                painter.text(
                    pos2(cell.left() + 6.0, cell.center().y),
                    Align2::LEFT_CENTER,
                    shown,
                    font.clone(),
                    visuals.text_color(),
                );
                if cursor == Some(at) {
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

        let page = to_steps((g.body_height() / g.row_height).floor());
        for (nav, extend) in ui.input_mut(take_navigation) {
            let Some(cursor) = self.cursor else { break };
            let target = match nav {
                Nav::Rows(d) => CellRef {
                    row: step(cursor.row, d, g.rows),
                    column: cursor.column,
                },
                Nav::Columns(d) => CellRef {
                    row: cursor.row,
                    column: step(cursor.column, d, g.columns),
                },
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

        let enter = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        if enter && let Some(cursor) = self.cursor {
            let sortable = source.column(cursor.column).is_some_and(|c| c.sortable);
            if sortable {
                intent = Some(sort_intent(commands, cursor.column, source.sorted()));
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
        self.scroll_y = scroll_to_show(
            self.scroll_y,
            g.body_height(),
            to_f32(cell.row) * g.row_height,
            g.row_height,
        );
        if cell.column >= g.frozen {
            let start = g.offsets.get(cell.column).copied().unwrap_or(0.0) - g.frozen_width;
            let length = self
                .widths
                .get(cell.column)
                .copied()
                .unwrap_or(DEFAULT_WIDTH);
            self.scroll_x = scroll_to_show(self.scroll_x, g.scroll_width, start, length);
        }
    }

    fn copy(&mut self, ui: &Ui, source: &dyn TableSource, g: &Geometry) {
        let Some(selection) = self.selection() else {
            return;
        };
        let rows = selection.rows.start.min(g.rows)..selection.rows.end.min(g.rows);
        let columns = selection.columns.start.min(g.columns)..selection.columns.end.min(g.columns);
        if rows.len().saturating_mul(columns.len()) > MAX_COPY_CELLS {
            self.notice = Some(messages::TABLE_COPY_TOO_LARGE);
            return;
        }
        self.notice = None;
        let cells = source.cells(rows, columns);
        ui.ctx().copy_text(format_grid(&cells));
    }

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
        let origin = self.selection().map_or(CellRef::default(), |s| CellRef {
            row: s.rows.start,
            column: s.columns.start,
        });
        let mut accepted = Vec::new();
        let mut refused = Vec::new();
        let mut refused_count = 0usize;
        for (dr, line) in grid.iter().enumerate() {
            for (dc, value) in line.iter().enumerate() {
                let cell = CellRef {
                    row: origin.row.saturating_add(dr),
                    column: origin.column.saturating_add(dc),
                };
                let in_table = cell.row < g.rows && cell.column < g.columns;
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

/// The intent of a sort click on `column`: ascending, or descending when the column is already
/// sorted ascending.
fn sort_intent(commands: &TableCommands, column: usize, current: Option<Sort>) -> Intent {
    let direction = match current {
        Some(s) if s.column == column && s.direction == SortDirection::Ascending => {
            SortDirection::Descending
        }
        _ => SortDirection::Ascending,
    };
    Intent::new(commands.sort.clone())
        .with("column", column)
        .with("direction", direction.as_str())
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
    reason = "display coordinates; above 2^24 points a position is rounded to about two points"
)]
fn to_f32(n: usize) -> f32 {
    n as f32
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

fn padded(range: Range<usize>, margin: usize, len: usize) -> Range<usize> {
    range.start.saturating_sub(margin)..range.end.saturating_add(margin).min(len)
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
