//! Tables (CORE-DRAFT-008): rows and columns of cells with typed content, merged ranges,
//! formulas, row types mapped to the fields of a table style, and breaking into column blocks.
//!
//! A table is an entity ([`TABLE`]). Its stored form is a list of column widths, a list of rows
//! (type and height), a sparse list of cells, a list of merged ranges and the breaking
//! parameters; [`Table`] is the same data as a value with the edit operations the commands
//! use. The evaluated cells (formula values, fields, number formats) are the derived node
//! [`RESOLVED`]; the display list reads it. The rules are stated in the crate specification,
//! section "Fields, tables and arrays as implemented".
//!
//! **Cell names.** A cell is named by its column letters and its row number: column 0 is `A`,
//! column 25 is `Z`, column 26 is `AA`; row 0 is `1`. Formulas use these names as the names of
//! the expression language (`B2 * 2 + sum([A1, A2])`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::math::Point2;
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{
    Bound, Cardinality, FieldDecl, Finding, Severity, Snapshot, TypeKey, TypeValidator, Value,
    ValueType,
};
use arc_geom::{Curve2, Segment2};

use crate::appearance::{Source, Sourced};
use crate::dimension::clearing_reference;
use crate::display::{
    DisplayOptions, Look, Primitive, Reader, Shape, lay_out, read, read_number, reference_list,
    text_primitives, text_style_of,
};
use crate::error::codes;
use crate::fields::{self, Field};
use crate::format::{NumberFormat, format_length};
use crate::style::{TABLE_STYLE, style_reference};
use crate::text::{Paragraph, Run, content_ok};
use crate::text_layout::{Columns, HAlign, TextInput, VAlign};
use crate::types::{
    BLOCK, TEXT_BYTES, common_findings, entity, finding, real, text, variants, with_kind,
};

/// A table: common fields, then the fields of [`field`].
pub const TABLE: &str = "core.draft.table";
/// The derived key of the evaluated cells of a table.
pub const RESOLVED: &str = "core.draft.table.resolved";

/// The most rows of a table.
pub const MAX_ROWS: usize = 1024;
/// The most columns of a table.
pub const MAX_COLUMNS: usize = 64;
/// The most stored cells of a table.
pub const MAX_CELLS: usize = 16_384;
/// The most merged ranges of a table.
pub const MAX_MERGES: usize = 1024;
/// What a cell whose formula fails shows.
pub const ERROR_MARKER: &str = "#ERR";

/// Field indices of a table.
pub mod field {
    use crate::types::entity::KIND as K;
    /// `position`: the top-left corner of the table.
    pub const POSITION: usize = K;
    /// `rotation` about the position, in radians.
    pub const ROTATION: usize = K + 1;
    /// `style`: the table style.
    pub const STYLE: usize = K + 2;
    /// `column_widths`.
    pub const COLUMNS: usize = K + 3;
    /// `rows`: type and height of each row.
    pub const ROWS: usize = K + 4;
    /// `cells`: the stored cells.
    pub const CELLS: usize = K + 5;
    /// `merges`: the merged ranges.
    pub const MERGES: usize = K + 6;
    /// `max_height`: the height at which the table breaks into a new column block; 0 for none.
    pub const MAX_HEIGHT: usize = K + 7;
    /// `break_gap`: the distance between column blocks.
    pub const BREAK_GAP: usize = K + 8;
}

const NAMES: [&str; 9] = [
    "position",
    "rotation",
    "style",
    "column_widths",
    "rows",
    "cells",
    "merges",
    "max_height",
    "break_gap",
];

/// The type of a row; it selects the row format of the table style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A title row (`title` of the table style).
    Title = 0,
    /// A header row (`header`), repeated at the top of every column block.
    Header = 1,
    /// A data row (`data`).
    Data = 2,
}

/// A row: its type and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// The type.
    pub kind: RowKind,
    /// The height.
    pub height: f64,
}

/// The content of a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Text with fields inserted.
    Text {
        /// The text.
        text: String,
        /// The fields inserted into it.
        fields: Vec<Field>,
    },
    /// A number.
    Number(f64),
    /// A formula: an `arc-solve` expression over cell names.
    Formula(String),
    /// A block definition drawn at the cell's alignment point.
    Block {
        /// The definition; absent when it was deleted.
        block: Option<ObjectId>,
        /// The uniform scale.
        scale: f64,
    },
}

/// A cell with its content.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The row, from 0.
    pub row: usize,
    /// The column, from 0.
    pub column: usize,
    /// The content.
    pub content: Content,
}

/// A merged range: the rectangle of `rows` by `columns` cells whose first cell is at (`row`,
/// `column`). Only the first cell holds content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Merge {
    /// The first row.
    pub row: usize,
    /// The first column.
    pub column: usize,
    /// The number of rows.
    pub rows: usize,
    /// The number of columns.
    pub columns: usize,
}

impl Merge {
    fn contains(&self, row: usize, column: usize) -> bool {
        (self.row..self.row + self.rows).contains(&row)
            && (self.column..self.column + self.columns).contains(&column)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.row < other.row + other.rows
            && other.row < self.row + self.rows
            && self.column < other.column + other.columns
            && other.column < self.column + self.columns
    }
}

/// The data of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The top-left corner.
    pub position: Point2,
    /// The rotation about the position.
    pub rotation: f64,
    /// The table style.
    pub style: Option<ObjectId>,
    /// The column widths.
    pub widths: Vec<f64>,
    /// The rows.
    pub rows: Vec<Row>,
    /// The cells, in no particular order.
    pub cells: Vec<Cell>,
    /// The merged ranges.
    pub merges: Vec<Merge>,
    /// The height at which a new column block starts; 0 for no breaking.
    pub max_height: f64,
    /// The distance between column blocks.
    pub break_gap: f64,
}

/// The name of column `column`: `A` to `Z`, then `AA`, `AB` and so on.
#[must_use]
pub fn column_name(column: usize) -> String {
    let mut n = column + 1;
    let mut letters = Vec::new();
    while n > 0 {
        let digit = u8::try_from((n - 1) % 26).unwrap_or(0);
        letters.push(char::from(b'A' + digit));
        n = (n - 1) / 26;
    }
    letters.iter().rev().collect()
}

/// The name of the cell at `row` and `column`, such as `B3`.
#[must_use]
pub fn cell_name(row: usize, column: usize) -> String {
    format!("{}{}", column_name(column), row + 1)
}

/// The (row, column) of a cell name: one to three capital letters and a row number from 1
/// without a leading zero.
#[must_use]
pub fn parse_cell(name: &str) -> Option<(usize, usize)> {
    let letters = name.chars().take_while(char::is_ascii_uppercase).count();
    let (head, tail) = (name.get(..letters)?, name.get(letters..)?);
    if !(1..=3).contains(&letters)
        || tail.is_empty()
        || tail.starts_with('0')
        || !tail.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let column = head
        .bytes()
        .fold(0_usize, |n, b| n * 26 + usize::from(b - b'A') + 1)
        - 1;
    let row = tail.parse::<usize>().ok()?.checked_sub(1)?;
    Some((row, column))
}

fn row_type() -> ValueType {
    ValueType::Record(vec![
        FieldDecl::new("kind", variants(&["title", "header", "data"])).with_default(Value::Enum(2)),
        real("height", 1.0, f64::MIN_POSITIVE, f64::MAX),
    ])
}

fn cell_type() -> ValueType {
    let index = |name: &str| {
        FieldDecl::new(name, ValueType::I64).with_bound(Bound::Int {
            min: 0,
            max: i64::try_from(MAX_ROWS).unwrap_or(i64::MAX),
        })
    };
    ValueType::Record(vec![
        index("row"),
        index("column"),
        FieldDecl::new("kind", variants(&["text", "number", "formula", "block"]))
            .with_default(Value::Enum(0)),
        text("text", TEXT_BYTES),
        fields::list_decl(),
        FieldDecl::new("number", ValueType::F64).optional(),
        text("formula", TEXT_BYTES),
        FieldDecl::new("block", clearing_reference(&[BLOCK], Cardinality::Optional)).optional(),
        real("scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
    ])
}

fn merge_type() -> ValueType {
    let count = |name: &str| {
        FieldDecl::new(name, ValueType::I64).with_bound(Bound::Int {
            min: 0,
            max: i64::try_from(MAX_ROWS).unwrap_or(i64::MAX),
        })
    };
    ValueType::Record(vec![
        count("row"),
        count("column"),
        count("row_span"),
        count("column_span"),
    ])
}

fn table_fields() -> Vec<FieldDecl> {
    let list = |name: &str, element: ValueType, max: usize| {
        FieldDecl::new(name, ValueType::List(Box::new(element))).with_bound(Bound::Length {
            max: u64::try_from(max).unwrap_or(u64::MAX),
        })
    };
    with_kind(vec![
        FieldDecl::new("position", ValueType::Point2).with_default(Value::Point2(Point2::origin())),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("style", style_reference(TABLE_STYLE, true)).optional(),
        list("column_widths", ValueType::F64, MAX_COLUMNS),
        list("rows", row_type(), MAX_ROWS),
        list("cells", cell_type(), MAX_CELLS),
        list("merges", merge_type(), MAX_MERGES),
        real("max_height", 0.0, 0.0, f64::MAX),
        real("break_gap", 0.0, 0.0, f64::MAX),
    ])
}

pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![(TABLE, table_fields(), vec![findings])]
}

fn findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut out = common_findings(values);
    if let Err(error) = Table::from_values(values).validate(&Limits::DEFAULT) {
        out.push(finding(error));
    }
    out
}

fn usize_at(f: &[Option<Value>], i: usize) -> Option<usize> {
    match f.get(i) {
        Some(Some(Value::I64(n))) => usize::try_from(*n).ok(),
        _ => None,
    }
}

impl Cell {
    fn to_value(&self) -> Value {
        let n = |x: usize| Some(Value::I64(i64::try_from(x).unwrap_or(i64::MAX)));
        let (kind, text, fields, number, formula, block, scale) = match &self.content {
            Content::Text { text, fields } => (
                0,
                Some(Value::String(text.clone())),
                fields::list_value(fields),
                None,
                None,
                None,
                1.0,
            ),
            Content::Number(x) => (
                1,
                None,
                Value::List(Vec::new()),
                Some(Value::F64(*x)),
                None,
                None,
                1.0,
            ),
            Content::Formula(f) => (
                2,
                None,
                Value::List(Vec::new()),
                None,
                Some(Value::String(f.clone())),
                None,
                1.0,
            ),
            Content::Block { block, scale } => (
                3,
                None,
                Value::List(Vec::new()),
                None,
                None,
                block.map(Value::Reference),
                *scale,
            ),
        };
        Value::Record(vec![
            n(self.row),
            n(self.column),
            Some(Value::Enum(kind)),
            text,
            Some(fields),
            number,
            formula,
            block,
            Some(Value::F64(scale)),
        ])
    }

    fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let string = |i: usize| match f.get(i) {
            Some(Some(Value::String(s))) => Some(s.clone()),
            _ => None,
        };
        let content = match f.get(2) {
            Some(Some(Value::Enum(0))) => Content::Text {
                text: string(3).unwrap_or_default(),
                fields: fields::fields_of(f.get(4).and_then(Option::as_ref)),
            },
            Some(Some(Value::Enum(1))) => match f.get(5) {
                Some(Some(Value::F64(x))) => Content::Number(*x),
                _ => return None,
            },
            Some(Some(Value::Enum(2))) => Content::Formula(string(6)?),
            Some(Some(Value::Enum(3))) => Content::Block {
                block: match f.get(7) {
                    Some(Some(Value::Reference(b))) => Some(*b),
                    _ => None,
                },
                scale: match f.get(8) {
                    Some(Some(Value::F64(x))) => *x,
                    _ => 1.0,
                },
            },
            _ => return None,
        };
        Some(Self {
            row: usize_at(f, 0)?,
            column: usize_at(f, 1)?,
            content,
        })
    }
}

impl Table {
    /// The table of the stored values of a table entity (all fields, common ones first).
    #[must_use]
    pub fn from_values(values: &[Option<Value>]) -> Self {
        let list = |i: usize| match values.get(i) {
            Some(Some(Value::List(items))) => items.as_slice(),
            _ => &[],
        };
        let real = |i: usize| match values.get(i) {
            Some(Some(Value::F64(x))) => *x,
            _ => 0.0,
        };
        Self {
            position: match values.get(field::POSITION) {
                Some(Some(Value::Point2(p))) => *p,
                _ => Point2::origin(),
            },
            rotation: real(field::ROTATION),
            style: match values.get(field::STYLE) {
                Some(Some(Value::Reference(s))) => Some(*s),
                _ => None,
            },
            widths: list(field::COLUMNS)
                .iter()
                .filter_map(|v| match v {
                    Value::F64(x) => Some(*x),
                    _ => None,
                })
                .collect(),
            rows: list(field::ROWS)
                .iter()
                .filter_map(|v| {
                    let Value::Record(f) = v else {
                        return None;
                    };
                    Some(Row {
                        kind: match f.first() {
                            Some(Some(Value::Enum(0))) => RowKind::Title,
                            Some(Some(Value::Enum(1))) => RowKind::Header,
                            _ => RowKind::Data,
                        },
                        height: match f.get(1) {
                            Some(Some(Value::F64(x))) => *x,
                            _ => return None,
                        },
                    })
                })
                .collect(),
            cells: list(field::CELLS)
                .iter()
                .filter_map(Cell::from_value)
                .collect(),
            merges: list(field::MERGES)
                .iter()
                .filter_map(|v| {
                    let Value::Record(f) = v else {
                        return None;
                    };
                    Some(Merge {
                        row: usize_at(f, 0)?,
                        column: usize_at(f, 1)?,
                        rows: usize_at(f, 2)?,
                        columns: usize_at(f, 3)?,
                    })
                })
                .collect(),
            max_height: real(field::MAX_HEIGHT),
            break_gap: real(field::BREAK_GAP),
        }
    }

    /// The stored lists that edits change, as (field name, value): `column_widths`, `rows`,
    /// `cells` and `merges`.
    #[must_use]
    pub fn stored(&self) -> Vec<(&'static str, Value)> {
        let n = |x: usize| Value::I64(i64::try_from(x).unwrap_or(i64::MAX));
        vec![
            (
                "column_widths",
                Value::List(self.widths.iter().map(|w| Value::F64(*w)).collect()),
            ),
            (
                "rows",
                Value::List(
                    self.rows
                        .iter()
                        .map(|r| {
                            Value::Record(vec![
                                Some(Value::Enum(r.kind as u32)),
                                Some(Value::F64(r.height)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "cells",
                Value::List(self.cells.iter().map(Cell::to_value).collect()),
            ),
            (
                "merges",
                Value::List(
                    self.merges
                        .iter()
                        .map(|m| {
                            Value::Record(vec![
                                Some(n(m.row)),
                                Some(n(m.column)),
                                Some(n(m.rows)),
                                Some(n(m.columns)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]
    }

    /// The merged range that holds the cell, if any.
    #[must_use]
    pub fn merge_at(&self, row: usize, column: usize) -> Option<&Merge> {
        self.merges.iter().find(|m| m.contains(row, column))
    }

    /// The content of the cell at (`row`, `column`).
    #[must_use]
    pub fn cell(&self, row: usize, column: usize) -> Option<&Cell> {
        self.cells
            .iter()
            .find(|c| c.row == row && c.column == column)
    }

    /// Checks the table against the rules.
    ///
    /// # Errors
    ///
    /// `draft.table.grid_invalid` for no rows or columns, or a size that is not finite and
    /// positive; `draft.table.too_large` for more rows, columns, cells or merged ranges than
    /// the bounds allow; `draft.table.merge_invalid` for a merged range that is not a
    /// rectangle of at least two cells inside the grid, or overlaps another;
    /// `draft.table.cell_invalid` for a cell outside the grid, stored twice, with content in a
    /// merged range but not at its first cell, or with content that is not valid;
    /// `draft.table.formula_invalid` for a formula that does not parse.
    pub fn validate(&self, limits: &Limits) -> Result<()> {
        let grid = |reason: &str| Error::new(codes::TABLE_GRID_INVALID).with_arg("reason", reason);
        let large = |what: &str| Error::new(codes::TABLE_TOO_LARGE).with_arg("limit", what);
        if self.rows.is_empty() || self.widths.is_empty() {
            return Err(grid("empty"));
        }
        if self.rows.len() > MAX_ROWS {
            return Err(large("rows"));
        }
        if self.widths.len() > MAX_COLUMNS {
            return Err(large("columns"));
        }
        if self.cells.len() > MAX_CELLS {
            return Err(large("cells"));
        }
        if self.merges.len() > MAX_MERGES {
            return Err(large("merges"));
        }
        let positive = |x: &f64| x.is_finite() && *x > 0.0;
        let not_negative = |x: f64| x.is_finite() && x >= 0.0;
        let sized = self.widths.iter().all(positive)
            && self.rows.iter().map(|r| &r.height).all(positive)
            && not_negative(self.max_height)
            && not_negative(self.break_gap)
            && self.rotation.is_finite()
            && self.position.x.is_finite()
            && self.position.y.is_finite();
        if !sized {
            return Err(grid("size"));
        }
        let (rows, columns) = (self.rows.len(), self.widths.len());
        let merge = || Error::new(codes::TABLE_MERGE_INVALID);
        for (i, m) in self.merges.iter().enumerate() {
            if m.rows < 1
                || m.columns < 1
                || m.rows * m.columns < 2
                || m.row + m.rows > rows
                || m.column + m.columns > columns
            {
                return Err(merge());
            }
            if self.merges.iter().skip(i + 1).any(|o| o.overlaps(m)) {
                return Err(merge());
            }
        }
        let cell = |row: usize, column: usize| {
            Error::new(codes::TABLE_CELL_INVALID).with_arg("cell", cell_name(row, column))
        };
        let mut seen = BTreeSet::new();
        let expression_limits = arc_solve::ExprLimits::DEFAULT.tightened_by(limits);
        for c in &self.cells {
            if c.row >= rows || c.column >= columns || !seen.insert((c.row, c.column)) {
                return Err(cell(c.row, c.column));
            }
            if self
                .merge_at(c.row, c.column)
                .is_some_and(|m| (m.row, m.column) != (c.row, c.column))
            {
                return Err(cell(c.row, c.column));
            }
            match &c.content {
                Content::Text { text, fields } => {
                    if !content_ok(text)
                        || u64::try_from(text.len()).is_ok_and(|n| n > limits.string_bytes)
                    {
                        return Err(cell(c.row, c.column));
                    }
                    fields::validate(text, fields, limits)?;
                }
                Content::Number(x) if !x.is_finite() => return Err(cell(c.row, c.column)),
                Content::Block { scale, .. } if !positive(scale) => {
                    return Err(cell(c.row, c.column));
                }
                Content::Formula(f)
                    if !content_ok(f)
                        || u64::try_from(f.len()).is_ok_and(|n| n > TEXT_BYTES)
                        || arc_solve::Expr::parse(f, &expression_limits).is_err() =>
                {
                    return Err(Error::new(codes::TABLE_FORMULA_INVALID)
                        .with_arg("cell", cell_name(c.row, c.column)));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Sets or, with `None`, clears the content of a cell. Content of a covered cell is
    /// refused by [`Table::validate`].
    pub fn set_cell(&mut self, row: usize, column: usize, content: Option<Content>) {
        self.cells.retain(|c| (c.row, c.column) != (row, column));
        if let Some(content) = content {
            self.cells.push(Cell {
                row,
                column,
                content,
            });
        }
    }

    /// Merges the rectangle from (`row0`, `column0`) to (`row1`, `column1`), corners in any
    /// order. The content of the other cells is dropped.
    ///
    /// # Errors
    ///
    /// `draft.table.merge_invalid` for a rectangle of one cell, outside the grid or
    /// overlapping a merged range.
    pub fn merge(
        &mut self,
        (row0, column0): (usize, usize),
        (row1, column1): (usize, usize),
    ) -> Result<()> {
        let m = Merge {
            row: row0.min(row1),
            column: column0.min(column1),
            rows: row0.abs_diff(row1) + 1,
            columns: column0.abs_diff(column1) + 1,
        };
        if m.rows * m.columns < 2
            || m.row + m.rows > self.rows.len()
            || m.column + m.columns > self.widths.len()
            || self.merges.iter().any(|o| o.overlaps(&m))
        {
            return Err(Error::new(codes::TABLE_MERGE_INVALID));
        }
        self.cells
            .retain(|c| !m.contains(c.row, c.column) || (c.row, c.column) == (m.row, m.column));
        self.merges.push(m);
        Ok(())
    }

    /// Removes the merged range that holds the cell; false when there is none.
    pub fn unmerge(&mut self, row: usize, column: usize) -> bool {
        let before = self.merges.len();
        self.merges.retain(|m| !m.contains(row, column));
        self.merges.len() != before
    }

    /// Inserts a row before `at` (`at` equal to the row count appends). A merged range that
    /// spans `at` grows.
    ///
    /// # Errors
    ///
    /// `draft.table.grid_invalid` when `at` is beyond the end.
    pub fn insert_row(&mut self, at: usize, row: Row) -> Result<()> {
        if at > self.rows.len() {
            return Err(Error::new(codes::TABLE_GRID_INVALID).with_arg("reason", "index"));
        }
        self.rows.insert(at, row);
        shift(&mut self.cells, &mut self.merges, Axis::Row, at, true);
        Ok(())
    }

    /// Inserts a column before `at`.
    ///
    /// # Errors
    ///
    /// `draft.table.grid_invalid` when `at` is beyond the end.
    pub fn insert_column(&mut self, at: usize, width: f64) -> Result<()> {
        if at > self.widths.len() {
            return Err(Error::new(codes::TABLE_GRID_INVALID).with_arg("reason", "index"));
        }
        self.widths.insert(at, width);
        shift(&mut self.cells, &mut self.merges, Axis::Column, at, true);
        Ok(())
    }

    /// Deletes a row with its cells; merged ranges across it shrink and one left with a single
    /// cell is removed.
    ///
    /// # Errors
    ///
    /// `draft.table.grid_invalid` when there is no such row.
    pub fn delete_row(&mut self, at: usize) -> Result<()> {
        if at >= self.rows.len() {
            return Err(Error::new(codes::TABLE_GRID_INVALID).with_arg("reason", "index"));
        }
        self.rows.remove(at);
        shift(&mut self.cells, &mut self.merges, Axis::Row, at, false);
        Ok(())
    }

    /// Deletes a column with its cells, as [`Table::delete_row`].
    ///
    /// # Errors
    ///
    /// `draft.table.grid_invalid` when there is no such column.
    pub fn delete_column(&mut self, at: usize) -> Result<()> {
        if at >= self.widths.len() {
            return Err(Error::new(codes::TABLE_GRID_INVALID).with_arg("reason", "index"));
        }
        self.widths.remove(at);
        shift(&mut self.cells, &mut self.merges, Axis::Column, at, false);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Row,
    Column,
}

/// Moves the cells and merged ranges after an insertion before, or a deletion of, the index
/// `at` on `axis`.
fn shift(cells: &mut Vec<Cell>, merges: &mut Vec<Merge>, axis: Axis, at: usize, insert: bool) {
    if !insert {
        cells.retain(|c| match axis {
            Axis::Row => c.row != at,
            Axis::Column => c.column != at,
        });
    }
    for c in cells.iter_mut() {
        let p = match axis {
            Axis::Row => &mut c.row,
            Axis::Column => &mut c.column,
        };
        if *p >= at && (insert || *p > at) {
            *p = if insert { *p + 1 } else { *p - 1 };
        }
    }
    for m in merges.iter_mut() {
        let (start, span) = match axis {
            Axis::Row => (&mut m.row, &mut m.rows),
            Axis::Column => (&mut m.column, &mut m.columns),
        };
        if insert {
            if *start >= at {
                *start += 1;
            } else if at < *start + *span {
                *span += 1;
            }
        } else if at < *start {
            *start -= 1;
        } else if at < *start + *span {
            *span -= 1;
        }
    }
    merges.retain(|m| m.rows * m.columns >= 2);
}

/// A row placed in a column block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The row.
    pub row: usize,
    /// The column block, from 0.
    pub segment: usize,
    /// The x offset of the block from the table's left edge.
    pub x: f64,
    /// The y of the row's top edge relative to the table's top edge (0 or negative).
    pub top: f64,
    /// Whether this is a repeated copy of a header row.
    pub repeated: bool,
}

/// Places the rows in column blocks. Rows run downwards from y = 0. When `max_height` is
/// positive, a data row that would reach below it, in a block that already holds a data row,
/// starts the next block at x = table width + `break_gap`, below the repeated header rows. The
/// rows of a vertically merged range stay together. The repeated rows are those of the groups
/// before the first data row that contain a header row.
#[must_use]
pub fn place_rows(table: &Table, tol: &Tolerance) -> Vec<Placed> {
    let n = table.rows.len();
    let height = |a: usize, b: usize| -> f64 {
        table
            .rows
            .iter()
            .skip(a)
            .take(b + 1 - a)
            .map(|r| r.height)
            .sum()
    };
    let first_data = table
        .rows
        .iter()
        .position(|r| r.kind == RowKind::Data)
        .unwrap_or(n);
    let mut groups = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = start;
        let mut changed = true;
        while changed {
            changed = false;
            for m in &table.merges {
                if m.row <= end && m.row + m.rows - 1 > end {
                    end = m.row + m.rows - 1;
                    changed = true;
                }
            }
        }
        let end = end.min(n - 1);
        groups.push((start, end));
        start = end + 1;
    }
    let repeated: Vec<(usize, usize)> = groups
        .iter()
        .copied()
        .filter(|(a, b)| {
            *a < first_data
                && table
                    .rows
                    .iter()
                    .skip(*a)
                    .take(b + 1 - a)
                    .any(|r| r.kind == RowKind::Header)
        })
        .collect();
    let width: f64 = table.widths.iter().sum();
    let (mut out, mut segment, mut x, mut y, mut flow) = (Vec::new(), 0, 0.0, 0.0_f64, 0_usize);
    let push =
        |out: &mut Vec<Placed>, (a, b): (usize, usize), segment, x, y: &mut f64, repeated| {
            for row in a..=b {
                out.push(Placed {
                    row,
                    segment,
                    x,
                    top: *y,
                    repeated,
                });
                *y -= height(row, row);
            }
        };
    for group in groups {
        let data = group.0 >= first_data;
        if data
            && table.max_height > 0.0
            && flow > 0
            && -y + height(group.0, group.1) > table.max_height + tol.linear
        {
            segment += 1;
            x += width + table.break_gap;
            y = 0.0;
            flow = 0;
            for r in &repeated {
                push(&mut out, *r, segment, x, &mut y, true);
            }
        }
        push(&mut out, group, segment, x, &mut y, false);
        if data {
            flow += 1;
        }
    }
    out
}

/// The evaluated cells of a table, the result of its [`RESOLVED`] node.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The shown text of every text, number and formula cell, by (row, column).
    pub texts: BTreeMap<(usize, usize), String>,
    /// The cells that show [`ERROR_MARKER`].
    pub errors: BTreeSet<(usize, usize)>,
    /// The cells that raise a warning.
    pub issues: Vec<Issue>,
}

/// A problem found while evaluating a table; each is a warning.
#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// A formula does not evaluate (`draft.table.formula_invalid`).
    Formula {
        /// The cell name.
        cell: String,
    },
    /// Formulas refer to each other (`draft.table.formula_cycle`); the cells are the ones in
    /// the cycle and the ones that depend on it.
    Cycle {
        /// The cell names in reading order.
        cells: Vec<String>,
    },
    /// A field in a cell shows the marker.
    Field {
        /// The cell name.
        cell: String,
        /// Why.
        kind: fields::IssueKind,
    },
}

impl Derived for Resolution {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text, as the display lists do.
        content_hash(&format!("{self:?}"))
    }
}

/// Formula cells waiting for evaluation: the parsed expression and the cells it names.
type Pending = BTreeMap<(usize, usize), (arc_solve::Expr, Vec<(usize, usize)>)>;

/// A cell value during evaluation.
#[derive(Clone)]
enum Cv {
    Number(f64),
    Text(String),
    Failed,
}

/// The cells named in `expr`.
fn names(expr: &arc_solve::Expr, out: &mut Vec<(usize, usize)>) {
    use arc_solve::ExprKind as K;
    match expr.kind() {
        K::Name(name) => out.extend(parse_cell(name)),
        K::List(items) | K::Call(_, items) => items.iter().for_each(|e| names(e, out)),
        K::Unary(_, e) => names(e, out),
        K::Binary(_, a, b) => {
            names(a, out);
            names(b, out);
        }
        K::Chain(first, rest) => {
            names(first, out);
            rest.iter().for_each(|(_, e)| names(e, out));
        }
        _ => {}
    }
}

/// The text of `value` read as a cell value.
fn cell_value(value: &arc_solve::Value) -> Cv {
    match value {
        arc_solve::Value::Text(t) => Cv::Text(t.to_string()),
        arc_solve::Value::Bool(b) => Cv::Text(b.to_string()),
        other => match other.quantity().map(|q| q.value()) {
            Some(x) if x.is_finite() => Cv::Number(x),
            _ => Cv::Failed,
        },
    }
}

/// The number format of `format`: a count of decimals from 0 to 8, or any other text for the
/// shortest form with up to 8 decimals.
fn number_text(x: f64, format: &str) -> String {
    let precision = format.trim().parse::<u32>().ok().filter(|p| *p <= 8);
    let f = NumberFormat {
        precision: precision.unwrap_or(8),
        suppress_trailing: precision.is_none(),
        ..NumberFormat::default()
    };
    format_length(x, &f, 1.0)
}

/// The row format of the table style for `kind`.
#[derive(Debug, Clone)]
struct RowStyle {
    text_style: Option<ObjectId>,
    text_height: f64,
    alignment: u32,
    borders: bool,
    format: String,
}

fn row_style(reader: &dyn Reader, style: Option<ObjectId>, kind: RowKind) -> Result<RowStyle> {
    let mut out = RowStyle {
        text_style: None,
        text_height: crate::text::DEFAULT_HEIGHT,
        alignment: 0,
        borders: true,
        format: String::new(),
    };
    let Some(style) = style else {
        return Ok(out);
    };
    let name = ["title", "header", "data"]
        .get(kind as usize)
        .copied()
        .unwrap_or("data");
    if let Some(Value::Record(f)) = reader.field(style, name)? {
        if let Some(Some(Value::Reference(t))) = f.first() {
            out.text_style = Some(*t);
        }
        if let Some(Some(Value::F64(h))) = f.get(1) {
            out.text_height = *h;
        }
        if let Some(Some(Value::Enum(a))) = f.get(2) {
            out.alignment = *a;
        }
        if let Some(Some(Value::Bool(b))) = f.get(3) {
            out.borders = *b;
        }
        if let Some(Some(Value::String(s))) = f.get(4) {
            out.format = s.clone();
        }
    }
    Ok(out)
}

fn read_table(reader: &dyn Reader, id: ObjectId) -> Result<Table> {
    let mut values = vec![None; entity::KIND];
    values.extend(read(reader, id, &NAMES)?);
    Ok(Table::from_values(&values))
}

/// Evaluates the cells of the table `id` through `reader`: numbers, text with its fields, and
/// formulas in dependency order. A formula that fails, or that is part of a cycle or reads a
/// failed cell, shows [`ERROR_MARKER`] and raises a warning; evaluation never recurses through
/// cell references and each formula is bounded, so a cycle cannot hang it.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over the cells in evaluation order"
)]
pub(crate) fn resolve_with(
    reader: &dyn Reader,
    id: ObjectId,
    tol: &Tolerance,
) -> Result<Resolution> {
    let table = read_table(reader, id)?;
    table.validate(&Limits::DEFAULT)?;
    let (rows, columns) = (table.rows.len(), table.widths.len());
    let mut values: BTreeMap<(usize, usize), Cv> = BTreeMap::new();
    let mut issues = Vec::new();
    let mut pending: Pending = BTreeMap::new();
    let limits = arc_solve::ExprLimits::DEFAULT.with_steps(100_000);
    let mut counted = 0;
    for c in &table.cells {
        let at = (c.row, c.column);
        match &c.content {
            Content::Number(x) => {
                values.insert(at, Cv::Number(*x));
            }
            Content::Text { text, fields } => {
                let mut found = Vec::new();
                let shown = fields::substitute(reader, id, text, fields, counted, tol, &mut found);
                counted += fields.len();
                issues.extend(found.into_iter().map(|i| Issue::Field {
                    cell: cell_name(c.row, c.column),
                    kind: i.kind,
                }));
                values.insert(at, Cv::Text(shown));
            }
            Content::Formula(f) => match arc_solve::Expr::parse(f, &limits) {
                Ok(expr) => {
                    let mut deps = Vec::new();
                    names(&expr, &mut deps);
                    pending.insert(at, (expr, deps));
                }
                Err(_) => {
                    values.insert(at, Cv::Failed);
                    issues.push(Issue::Formula {
                        cell: cell_name(c.row, c.column),
                    });
                }
            },
            Content::Block { .. } => {}
        }
    }
    // Kahn's algorithm over the formula cells: a cell is ready when no formula it reads is
    // still pending.
    let mut order = Vec::new();
    let mut remaining: BTreeSet<(usize, usize)> = pending.keys().copied().collect();
    loop {
        let ready: Vec<(usize, usize)> = remaining
            .iter()
            .copied()
            .filter(|at| {
                pending
                    .get(at)
                    .is_some_and(|(_, deps)| deps.iter().all(|d| !remaining.contains(d)))
            })
            .collect();
        if ready.is_empty() {
            break;
        }
        for at in ready {
            remaining.remove(&at);
            order.push(at);
        }
    }
    for at in order {
        let Some((expr, _)) = pending.get(&at) else {
            continue;
        };
        let value = {
            let lookup = |name: &str| -> Result<Option<arc_solve::Value>> {
                let Some(cell) = parse_cell(name) else {
                    return Ok(None);
                };
                if cell.0 >= rows || cell.1 >= columns {
                    return Err(Error::new(codes::TABLE_FORMULA_INVALID));
                }
                match values.get(&cell) {
                    Some(Cv::Number(x)) => Ok(Some(arc_solve::Value::Bare(*x))),
                    Some(Cv::Text(t)) => Ok(Some(arc_solve::Value::text(t.as_str()))),
                    Some(Cv::Failed) => Err(Error::new(codes::TABLE_FORMULA_INVALID)),
                    None => Ok(Some(arc_solve::Value::Bare(0.0))),
                }
            };
            let context = arc_solve::Context::new()
                .with_limits(limits)
                .with_names(&lookup);
            expr.evaluate(&context)
        };
        match value.as_ref().map(cell_value) {
            Ok(v @ (Cv::Number(_) | Cv::Text(_))) => {
                values.insert(at, v);
            }
            _ => {
                values.insert(at, Cv::Failed);
                issues.push(Issue::Formula {
                    cell: cell_name(at.0, at.1),
                });
            }
        }
    }
    if !remaining.is_empty() {
        let mut cells: Vec<String> = Vec::new();
        for at in &remaining {
            values.insert(*at, Cv::Failed);
            cells.push(cell_name(at.0, at.1));
        }
        issues.push(Issue::Cycle { cells });
    }
    let mut texts = BTreeMap::new();
    let mut errors = BTreeSet::new();
    for (at, value) in values {
        let kind = table.rows.get(at.0).map_or(RowKind::Data, |r| r.kind);
        let format = row_style(reader, table.style, kind)?.format;
        match value {
            Cv::Number(x) => texts.insert(at, number_text(x, &format)),
            Cv::Text(t) => texts.insert(at, t),
            Cv::Failed => {
                errors.insert(at);
                texts.insert(at, ERROR_MARKER.to_owned())
            }
        };
    }
    Ok(Resolution {
        texts,
        errors,
        issues,
    })
}

/// The node of the evaluated cells of the table `id`.
///
/// # Errors
///
/// None for the constant key; the key check of `arc-doc` is passed on.
pub fn resolved_node(id: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(id, DerivedKey::new(RESOLVED)?))
}

/// Registers the [`RESOLVED`] evaluator of tables with the tolerance of `options`.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when it is registered already.
pub fn register_evaluators(evaluators: &mut Evaluators, options: &DisplayOptions) -> Result<()> {
    let tol = options.tolerance;
    evaluators.register_object(
        TypeKey::new(TABLE)?,
        DerivedKey::new(RESOLVED)?,
        move |ctx: &EvalContext<'_>| {
            let id = ctx
                .object_id()
                .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))?;
            let resolution = resolve_with(&crate::display::GraphReader { ctx }, id, &tol)?;
            Ok(Arc::new(resolution) as DerivedValue)
        },
    )
}

/// The evaluated cells of the table `id` of `snapshot`, without the graph.
///
/// # Errors
///
/// `doc.object.not_found`; the validation errors of [`Table::validate`].
pub fn resolve(snapshot: &Snapshot, id: ObjectId, options: &DisplayOptions) -> Result<Resolution> {
    crate::display::with_snapshot(snapshot, options, |reader| {
        resolve_with(reader, id, &options.tolerance)
    })
}

/// A warning for every table of `snapshot` that has a formula or field problem.
pub(crate) fn diagnostics(snapshot: &Snapshot) -> Vec<Finding> {
    let Ok(all) = snapshot.iter_type(TABLE) else {
        return Vec::new();
    };
    let options = crate::dimension::resolution_options();
    let mut out = Vec::new();
    for (_, object) in all {
        let Ok(resolution) = resolve(snapshot, object.id, &options) else {
            continue;
        };
        for issue in resolution.issues {
            let error = match issue {
                Issue::Formula { cell } => {
                    Error::new(codes::TABLE_FORMULA_INVALID).with_arg("cell", cell)
                }
                Issue::Cycle { cells } => {
                    Error::new(codes::TABLE_FORMULA_CYCLE).with_arg("cells", cells.join(", "))
                }
                Issue::Field { cell, kind } => kind.error().with_detail("cell", cell),
            };
            out.push(Finding {
                severity: Severity::Warning,
                error: error.with_detail("id", object.id.to_string()),
            });
        }
    }
    out
}

fn continuous() -> Option<Sourced<Option<ObjectId>>> {
    Some(Sourced {
        value: None,
        source: Source::Object,
    })
}

fn too_large(options: &DisplayOptions) -> Error {
    Error::new(codes::DISPLAY_TOO_LARGE).with_arg(
        "limit",
        i64::try_from(options.max_primitives).unwrap_or(i64::MAX),
    )
}

/// The primitives of the table `id`: one segment for every cell edge that is not inside a
/// merged range (part 0), the text of every cell (part 1) and the content of block cells
/// (part 2).
#[expect(
    clippy::too_many_lines,
    reason = "grid lines, then the content of every cell"
)]
pub(crate) fn primitives(
    reader: &dyn Reader,
    id: ObjectId,
    base: &Primitive,
    look: &Look,
    options: &DisplayOptions,
) -> Result<Vec<Primitive>> {
    let table = read_table(reader, id)?;
    table.validate(&arc_base::Limits::DEFAULT)?;
    let resolution = reader.table(id)?;
    let placed = place_rows(&table, &options.tolerance);
    let (sin, cos) = table.rotation.sin_cos();
    let point = |x: f64, y: f64| {
        Point2::new(
            table.position.x + cos * x - sin * y,
            table.position.y + sin * x + cos * y,
        )
    };
    let mut column_x = vec![0.0];
    for w in &table.widths {
        column_x.push(column_x.last().copied().unwrap_or(0.0) + w);
    }
    let x_at = |c: usize| column_x.get(c).copied().unwrap_or(0.0);
    let height = |r: usize, n: usize| -> f64 {
        table
            .rows
            .iter()
            .skip(r)
            .take(n)
            .map(|row| row.height)
            .sum()
    };
    let kind_of = |r: usize| table.rows.get(r).map_or(RowKind::Data, |row| row.kind);
    let styles =
        [RowKind::Title, RowKind::Header, RowKind::Data].map(|k| row_style(reader, table.style, k));
    let [title, header, data] = styles;
    let styles = [title?, header?, data?];
    let style_of = |r: usize| styles.get(kind_of(r) as usize).cloned();
    let margin = match table.style {
        Some(s) => read_number(reader, s, "cell_margin")?.unwrap_or(1.0),
        None => 1.0,
    };
    let line = Primitive {
        linetype: continuous(),
        part: 0,
        ..base.clone()
    };
    let mut out: Vec<Primitive> = Vec::new();
    let edge =
        |out: &mut Vec<Primitive>, (x0, y0): (f64, f64), (x1, y1): (f64, f64)| -> Result<()> {
            let segment = Segment2::new(point(x0, y0), point(x1, y1))?;
            out.push(Primitive {
                shape: Shape::Stroke {
                    curve: Curve2::Segment(segment),
                    drawn: None,
                },
                ..line.clone()
            });
            Ok(())
        };
    let same_merge = |r0: usize, r1: usize, c0: usize, c1: usize| {
        table.merge_at(r0, c0).is_some_and(|m| m.contains(r1, c1))
    };
    let borders = |r: usize| style_of(r).is_some_and(|s| s.borders);
    let columns = table.widths.len();
    let mut previous: Option<&Placed> = None;
    for p in &placed {
        let x = p.x;
        let top = p.top;
        let bottom = top - height(p.row, 1);
        // The edge above the row: shared with the row above it in the same block.
        let above = previous.filter(|q| q.segment == p.segment);
        if above.map_or_else(|| borders(p.row), |q| borders(q.row) || borders(p.row)) {
            for c in 0..columns {
                if above.is_none_or(|q| !same_merge(q.row, p.row, c, c)) {
                    edge(&mut out, (x + x_at(c), top), (x + x_at(c + 1), top))?;
                }
            }
        }
        if borders(p.row) {
            for c in 0..=columns {
                if c == 0 || c == columns || !same_merge(p.row, p.row, c - 1, c) {
                    edge(&mut out, (x + x_at(c), top), (x + x_at(c), bottom))?;
                }
            }
        }
        // The edge below the last row of a block.
        let last = placed
            .iter()
            .rfind(|q| q.segment == p.segment)
            .is_some_and(|q| q.row == p.row && q.repeated == p.repeated);
        if last && borders(p.row) {
            for c in 0..columns {
                edge(&mut out, (x + x_at(c), bottom), (x + x_at(c + 1), bottom))?;
            }
        }
        previous = Some(p);
        if out.len() > options.max_primitives {
            return Err(too_large(options));
        }
    }
    let mut by_row: BTreeMap<usize, Vec<&Cell>> = BTreeMap::new();
    for c in &table.cells {
        by_row.entry(c.row).or_default().push(c);
    }
    for p in &placed {
        let Some(style) = style_of(p.row) else {
            continue;
        };
        for c in by_row.get(&p.row).into_iter().flatten() {
            let span = table.merge_at(c.row, c.column);
            let (rows, cols) = span.map_or((1, 1), |m| (m.rows, m.columns));
            let (x0, x1) = (p.x + x_at(c.column), p.x + x_at(c.column + cols));
            let middle = p.top - height(c.row, rows) * 0.5;
            let anchor_x = match style.alignment {
                1 => (x0 + x1) * 0.5,
                2 => x1 - margin,
                _ => x0 + margin,
            };
            let anchor = point(anchor_x, middle);
            match &c.content {
                Content::Block {
                    block: Some(block),
                    scale,
                } => {
                    let mut values = vec![None; entity::KIND];
                    values.extend([
                        Some(Value::Reference(*block)),
                        Some(Value::Point2(anchor)),
                        Some(Value::Vector3(arc_base::math::Vector3::new(
                            *scale, *scale, 1.0,
                        ))),
                        Some(Value::F64(table.rotation)),
                    ]);
                    out.extend(
                        reference_list(reader, &values, base.clone(), look, false, options)?
                            .primitives
                            .into_iter()
                            .map(|p| Primitive { part: 2, ..p }),
                    );
                }
                Content::Block { .. } => {}
                _ => {
                    let Some(shown) = resolution.texts.get(&(c.row, c.column)) else {
                        continue;
                    };
                    if shown.is_empty() {
                        continue;
                    }
                    let style_height = match style.text_style {
                        Some(s) => read_number(reader, s, "height")?.unwrap_or(0.0),
                        None => 0.0,
                    };
                    let text_height =
                        [style.text_height, style_height, crate::text::DEFAULT_HEIGHT]
                            .into_iter()
                            .find(|h| *h > 0.0)
                            .unwrap_or(crate::text::DEFAULT_HEIGHT);
                    let text_style = text_style_of(reader, style.text_style)?;
                    let paragraphs = [Paragraph {
                        runs: vec![Run {
                            text: shown.clone(),
                            ..Run::default()
                        }],
                        ..Paragraph::default()
                    }];
                    let input = TextInput {
                        paragraphs: &paragraphs,
                        height: text_height,
                        width: 0.0,
                        halign: match style.alignment {
                            1 => HAlign::Center,
                            2 => HAlign::Right,
                            _ => HAlign::Left,
                        },
                        valign: VAlign::Middle,
                        columns: Columns::default(),
                        style: &text_style,
                    };
                    let layout = lay_out(reader, &input, options)?;
                    let frame = arc_base::math::Transform2::from_matrix_unchecked(
                        arc_base::math::Matrix3::new(
                            cos, -sin, anchor.x, sin, cos, anchor.y, 0.0, 0.0, 1.0,
                        ),
                    );
                    let template = Primitive {
                        part: 1,
                        ..base.clone()
                    };
                    out.extend(text_primitives(
                        &layout,
                        &frame,
                        &template,
                        &options.tolerance,
                    )?);
                }
            }
            if out.len() > options.max_primitives {
                return Err(too_large(options));
            }
        }
    }
    Ok(out)
}
