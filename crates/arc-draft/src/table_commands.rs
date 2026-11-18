//! The commands of tables: creation, cell content, rows, columns and merged ranges.

use arc_base::{Error, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{FieldPath, Value};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{Created, create_entity, expect, family, point2, spec, values_of};
use crate::error::codes;
use crate::fields::Field;
use crate::style::TABLE_STYLE;
use crate::table::{Content, Row, RowKind, TABLE, Table, field, parse_cell};
use crate::types::{BLOCK, CONTAINERS};

/// The type of a row.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowKindParam {
    /// A title row.
    Title,
    /// A header row.
    Header,
    /// A data row.
    #[default]
    Data,
}

impl RowKindParam {
    fn kind(self) -> RowKind {
        match self {
            Self::Title => RowKind::Title,
            Self::Header => RowKind::Header,
            Self::Data => RowKind::Data,
        }
    }
}

/// A new row.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowParam {
    /// The type; default `data`.
    #[serde(default)]
    pub kind: RowKindParam,
    /// The height.
    pub height: f64,
}

/// The content of a cell.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentParam {
    /// Text, with fields inserted.
    Text {
        /// The text.
        text: String,
        /// The fields.
        #[serde(default)]
        fields: Vec<Field>,
    },
    /// A number.
    Number {
        /// The value.
        value: f64,
    },
    /// A formula over cell names.
    Formula {
        /// The expression.
        expression: String,
    },
    /// A block definition.
    Block {
        /// The definition.
        block: IdParam,
        /// The uniform scale; default 1.
        #[serde(default)]
        scale: Option<f64>,
    },
    /// No content.
    Empty,
}

impl ContentParam {
    fn content(self) -> Option<Content> {
        match self {
            Self::Text { text, fields } => Some(Content::Text { text, fields }),
            Self::Number { value } => Some(Content::Number(value)),
            Self::Formula { expression } => Some(Content::Formula(expression)),
            Self::Block { block, scale } => Some(Content::Block {
                block: Some(block.0),
                scale: scale.unwrap_or(1.0),
            }),
            Self::Empty => None,
        }
    }
}

/// A cell and its content.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CellParam {
    /// The cell name, such as `B3`.
    pub cell: String,
    /// The content.
    pub content: ContentParam,
}

/// Parameters of `core.draft.table.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTable {
    /// The space or block definition.
    pub owner: IdParam,
    /// The column widths.
    #[schemars(length(min = 1, max = 64))]
    pub columns: Vec<f64>,
    /// The rows.
    #[schemars(length(min = 1, max = 1024))]
    pub rows: Vec<RowParam>,
    /// The cells with content.
    #[serde(default)]
    #[schemars(length(max = 16384))]
    pub cells: Vec<CellParam>,
    /// The top-left corner; default (0, 0).
    #[serde(default)]
    pub position: [f64; 2],
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The table style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The height at which a new column block starts; 0 or absent for no breaking.
    #[serde(default)]
    pub max_height: f64,
    /// The distance between column blocks.
    #[serde(default)]
    pub break_gap: f64,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `core.draft.table.cell.set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCell {
    /// The table.
    pub table: IdParam,
    /// The cell name.
    pub cell: String,
    /// The content; `empty` clears the cell.
    pub content: ContentParam,
}

/// Parameters of `core.draft.table.row.insert`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertRow {
    /// The table.
    pub table: IdParam,
    /// The index the new row takes, from 0; the row count appends.
    pub at: usize,
    /// The type; default `data`.
    #[serde(default)]
    pub kind: RowKindParam,
    /// The height.
    pub height: f64,
}

/// Parameters of `core.draft.table.column.insert`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertColumn {
    /// The table.
    pub table: IdParam,
    /// The index the new column takes, from 0; the column count appends.
    pub at: usize,
    /// The width.
    pub width: f64,
}

/// Parameters of `core.draft.table.row.delete` and `core.draft.table.column.delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteIndex {
    /// The table.
    pub table: IdParam,
    /// The index of the row or column, from 0.
    pub at: usize,
}

/// Parameters of `core.draft.table.merge` and `core.draft.table.unmerge`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeRange {
    /// The table.
    pub table: IdParam,
    /// `merge`: the range, such as `A1:C2`. `unmerge`: any cell of the merged range.
    pub range: String,
}

fn cell_of(name: &str) -> Result<(usize, usize)> {
    parse_cell(name).ok_or_else(|| Error::new(codes::TABLE_CELL_INVALID).with_arg("cell", name))
}

/// The table `id`, read from the transaction.
fn load(ctx: &mut Context<'_>, id: ObjectId) -> Result<Table> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, id, &[TABLE])?;
    Ok(Table::from_values(&values_of(&snapshot, id)?))
}

/// Validates `table` and stores its lists.
fn store(ctx: &mut Context<'_>, id: ObjectId, table: &Table) -> Result<()> {
    table.validate(ctx.limits())?;
    let txn = ctx.txn()?;
    for (name, value) in table.stored() {
        txn.modify(id, FieldPath::new(name), Some(value))?;
    }
    Ok(())
}

fn create(ctx: &mut Context<'_>, p: CreateTable) -> Result<Created> {
    let mut table = Table::from_values(&[]);
    table.widths = p.columns;
    table.rows = p
        .rows
        .iter()
        .map(|r| Row {
            kind: r.kind.kind(),
            height: r.height,
        })
        .collect();
    for c in p.cells {
        let (row, column) = cell_of(&c.cell)?;
        table.set_cell(row, column, c.content.content());
    }
    table.validate(ctx.limits())?;
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.owner.0, CONTAINERS)?;
    let mut kind: Vec<(usize, Value)> = vec![
        (field::POSITION, point2(p.position)),
        (field::ROTATION, Value::F64(p.rotation)),
        (field::MAX_HEIGHT, Value::F64(p.max_height)),
        (field::BREAK_GAP, Value::F64(p.break_gap)),
    ];
    if let Some(IdParam(style)) = p.style {
        expect(&snapshot, style, &[TABLE_STYLE])?;
        kind.push((field::STYLE, Value::Reference(style)));
    }
    let stored = table.stored();
    for (index, (_, value)) in [field::COLUMNS, field::ROWS, field::CELLS, field::MERGES]
        .into_iter()
        .zip(stored)
    {
        kind.push((index, value));
    }
    create_entity(ctx, TABLE, p.owner.0, p.layer, kind)
}

fn set_cell(ctx: &mut Context<'_>, p: SetCell) -> Result<()> {
    let (row, column) = cell_of(&p.cell)?;
    let mut table = load(ctx, p.table.0)?;
    if let ContentParam::Block { block, .. } = &p.content {
        let snapshot = ctx.txn()?.snapshot().clone();
        expect(&snapshot, block.0, &[BLOCK])?;
    }
    table.set_cell(row, column, p.content.content());
    store(ctx, p.table.0, &table)
}

fn edit(
    ctx: &mut Context<'_>,
    id: IdParam,
    change: impl FnOnce(&mut Table) -> Result<()>,
) -> Result<()> {
    let mut table = load(ctx, id.0)?;
    change(&mut table)?;
    store(ctx, id.0, &table)
}

fn merge(ctx: &mut Context<'_>, p: MergeRange) -> Result<()> {
    let (a, b) = p.range.split_once(':').unwrap_or((&p.range, &p.range));
    let (a, b) = (cell_of(a)?, cell_of(b)?);
    edit(ctx, p.table, |t| t.merge(a, b))
}

fn unmerge(ctx: &mut Context<'_>, p: MergeRange) -> Result<()> {
    let (row, column) = cell_of(&p.range)?;
    edit(ctx, p.table, |t| {
        if t.unmerge(row, column) {
            Ok(())
        } else {
            Err(Error::new(codes::TABLE_MERGE_INVALID))
        }
    })
}

/// The commands of this module.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.table.create", create),
        family("core.draft.table.modify", &[TABLE]),
        spec("core.draft.table.cell.set", set_cell),
        spec("core.draft.table.row.insert", |ctx, p: InsertRow| {
            edit(ctx, p.table, |t| {
                t.insert_row(
                    p.at,
                    Row {
                        kind: p.kind.kind(),
                        height: p.height,
                    },
                )
            })
        }),
        spec("core.draft.table.row.delete", |ctx, p: DeleteIndex| {
            edit(ctx, p.table, |t| t.delete_row(p.at))
        }),
        spec("core.draft.table.column.insert", |ctx, p: InsertColumn| {
            edit(ctx, p.table, |t| t.insert_column(p.at, p.width))
        }),
        spec("core.draft.table.column.delete", |ctx, p: DeleteIndex| {
            edit(ctx, p.table, |t| t.delete_column(p.at))
        }),
        spec("core.draft.table.merge", merge),
        spec("core.draft.table.unmerge", unmerge),
    ]
}
