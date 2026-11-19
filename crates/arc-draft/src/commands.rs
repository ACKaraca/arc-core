//! The drafting commands (`core.draft.*`), registered with [`register_commands`].
//!
//! Every command is undoable, has the permission class `edit` and changes the document only
//! through its transaction. Creating commands select and return the new object. Modify commands
//! set one field of one or more objects from its canonical JSON form ([`field_value`]); `null`
//! makes an optional field absent. Each modify command accepts only objects of its family.

use std::collections::BTreeMap;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Vector3};
use arc_base::{Color, Error, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam, Permission, Registry, UndoBehavior};
use arc_doc::relationship::ReferenceField;
use arc_doc::{
    ColorSource, ColorValue, FieldPath, GeometryBytes, Header, Snapshot, Value, ValueType,
};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dimension::DIMENSION;
use crate::error::codes;
use crate::fonts::{FONT_TABLE, document_table, font_table_id};
use crate::layout::{LAYOUT, Orientation, VIEWPORT, viewport};
use crate::order::{Place, reorder};
use crate::style::{DEFAULT_NAME, DIM_STYLE, LEADER_STYLE, STYLES, TABLE_STYLE, TEXT_STYLE};
use crate::text::{
    Paragraph, RICH_TEXT, TEXT, field as text_field, paragraphs_from_plain, paragraphs_value,
    validate as validate_paragraphs,
};
use crate::types::{
    self, BLOCK, BLOCK_REF, CONTAINERS, ENTITIES, ElementKind, LAYER, LINETYPE, SPACE, SpaceKind,
    SpaceUnit, entity, layer, table,
};

/// The deepest nesting of lists, maps and records [`field_value`] reads.
const MAX_JSON_DEPTH: u32 = 16;

struct Names {
    id: &'static str,
    message_id: &'static str,
    description: Description,
}

struct Description {
    message_id: &'static str,
}

const NAMES: &[Names] = &[
    Names {
        id: "core.draft.space.create",
        message_id: "draft-command-space-create",
        description: Description {
            message_id: "draft-command-space-create-description",
        },
    },
    Names {
        id: "core.draft.space.modify",
        message_id: "draft-command-space-modify",
        description: Description {
            message_id: "draft-command-space-modify-description",
        },
    },
    Names {
        id: "core.draft.entity.create",
        message_id: "draft-command-entity-create",
        description: Description {
            message_id: "draft-command-entity-create-description",
        },
    },
    Names {
        id: "core.draft.spline.create",
        message_id: "draft-command-spline-create",
        description: Description {
            message_id: "draft-command-spline-create-description",
        },
    },
    Names {
        id: "core.draft.spline.set",
        message_id: "draft-command-spline-set",
        description: Description {
            message_id: "draft-command-spline-set-description",
        },
    },
    Names {
        id: "core.draft.plot_style_table.create",
        message_id: "draft-command-plot-style-table-create",
        description: Description {
            message_id: "draft-command-plot-style-table-create-description",
        },
    },
    Names {
        id: "core.draft.plot_style_table.modify",
        message_id: "draft-command-plot-style-table-modify",
        description: Description {
            message_id: "draft-command-plot-style-table-modify-description",
        },
    },
    Names {
        id: "core.draft.plot_style_table.delete",
        message_id: "draft-command-plot-style-table-delete",
        description: Description {
            message_id: "draft-command-plot-style-table-delete-description",
        },
    },
    Names {
        id: "core.draft.entity.modify",
        message_id: "draft-command-entity-modify",
        description: Description {
            message_id: "draft-command-entity-modify-description",
        },
    },
    Names {
        id: "core.draft.entity.delete",
        message_id: "draft-command-entity-delete",
        description: Description {
            message_id: "draft-command-entity-delete-description",
        },
    },
    Names {
        id: "core.draft.layer.create",
        message_id: "draft-command-layer-create",
        description: Description {
            message_id: "draft-command-layer-create-description",
        },
    },
    Names {
        id: "core.draft.layer.modify",
        message_id: "draft-command-layer-modify",
        description: Description {
            message_id: "draft-command-layer-modify-description",
        },
    },
    Names {
        id: "core.draft.layer.delete",
        message_id: "draft-command-layer-delete",
        description: Description {
            message_id: "draft-command-layer-delete-description",
        },
    },
    Names {
        id: "core.draft.linetype.load",
        message_id: "draft-command-linetype-load",
        description: Description {
            message_id: "draft-command-linetype-load-description",
        },
    },
    Names {
        id: "core.draft.linetype.modify",
        message_id: "draft-command-linetype-modify",
        description: Description {
            message_id: "draft-command-linetype-modify-description",
        },
    },
    Names {
        id: "core.draft.block.define",
        message_id: "draft-command-block-define",
        description: Description {
            message_id: "draft-command-block-define-description",
        },
    },
    Names {
        id: "core.draft.block.modify",
        message_id: "draft-command-block-modify",
        description: Description {
            message_id: "draft-command-block-modify-description",
        },
    },
    Names {
        id: "core.draft.external_ref.create",
        message_id: "draft-command-external-ref-create",
        description: Description {
            message_id: "draft-command-external-ref-create-description",
        },
    },
    Names {
        id: "core.draft.external_ref.set_clip",
        message_id: "draft-command-external-ref-set-clip",
        description: Description {
            message_id: "draft-command-external-ref-set-clip-description",
        },
    },
    Names {
        id: "core.draft.external_ref.bind",
        message_id: "draft-command-external-ref-bind",
        description: Description {
            message_id: "draft-command-external-ref-bind-description",
        },
    },
    Names {
        id: "core.draft.block.insert",
        message_id: "draft-command-block-insert",
        description: Description {
            message_id: "draft-command-block-insert-description",
        },
    },
    Names {
        id: "core.draft.order.front",
        message_id: "draft-command-order-front",
        description: Description {
            message_id: "draft-command-order-front-description",
        },
    },
    Names {
        id: "core.draft.order.back",
        message_id: "draft-command-order-back",
        description: Description {
            message_id: "draft-command-order-back-description",
        },
    },
    Names {
        id: "core.draft.order.above",
        message_id: "draft-command-order-above",
        description: Description {
            message_id: "draft-command-order-above-description",
        },
    },
    Names {
        id: "core.draft.order.below",
        message_id: "draft-command-order-below",
        description: Description {
            message_id: "draft-command-order-below-description",
        },
    },
    Names {
        id: "core.draft.layout.create",
        message_id: "draft-command-layout-create",
        description: Description {
            message_id: "draft-command-layout-create-description",
        },
    },
    Names {
        id: "core.draft.layout.modify",
        message_id: "draft-command-layout-modify",
        description: Description {
            message_id: "draft-command-layout-modify-description",
        },
    },
    Names {
        id: "core.draft.viewport.create",
        message_id: "draft-command-viewport-create",
        description: Description {
            message_id: "draft-command-viewport-create-description",
        },
    },
    Names {
        id: "core.draft.viewport.modify",
        message_id: "draft-command-viewport-modify",
        description: Description {
            message_id: "draft-command-viewport-modify-description",
        },
    },
    Names {
        id: "core.draft.viewport.delete",
        message_id: "draft-command-viewport-delete",
        description: Description {
            message_id: "draft-command-viewport-delete-description",
        },
    },
    Names {
        id: "core.draft.wipeout.create",
        message_id: "draft-command-wipeout-create",
        description: Description {
            message_id: "draft-command-wipeout-create-description",
        },
    },
    Names {
        id: "core.draft.style.create",
        message_id: "draft-command-style-create",
        description: Description {
            message_id: "draft-command-style-create-description",
        },
    },
    Names {
        id: "core.draft.style.modify",
        message_id: "draft-command-style-modify",
        description: Description {
            message_id: "draft-command-style-modify-description",
        },
    },
    Names {
        id: "core.draft.style.delete",
        message_id: "draft-command-style-delete",
        description: Description {
            message_id: "draft-command-style-delete-description",
        },
    },
    Names {
        id: "core.draft.style.defaults",
        message_id: "draft-command-style-defaults",
        description: Description {
            message_id: "draft-command-style-defaults-description",
        },
    },
    Names {
        id: "core.draft.text.create",
        message_id: "draft-command-text-create",
        description: Description {
            message_id: "draft-command-text-create-description",
        },
    },
    Names {
        id: "core.draft.rich_text.create",
        message_id: "draft-command-rich-text-create",
        description: Description {
            message_id: "draft-command-rich-text-create-description",
        },
    },
    Names {
        id: "core.draft.rich_text.set",
        message_id: "draft-command-rich-text-set",
        description: Description {
            message_id: "draft-command-rich-text-set-description",
        },
    },
    Names {
        id: "core.draft.font.substitute",
        message_id: "draft-command-font-substitute",
        description: Description {
            message_id: "draft-command-font-substitute-description",
        },
    },
    Names {
        id: "core.draft.hatch.create",
        message_id: "draft-command-hatch-create",
        description: Description {
            message_id: "draft-command-hatch-create-description",
        },
    },
    Names {
        id: "core.draft.hatch.pick",
        message_id: "draft-command-hatch-pick",
        description: Description {
            message_id: "draft-command-hatch-pick-description",
        },
    },
    Names {
        id: "core.draft.hatch.modify",
        message_id: "draft-command-hatch-modify",
        description: Description {
            message_id: "draft-command-hatch-modify-description",
        },
    },
    Names {
        id: "core.draft.hatch_pattern.load",
        message_id: "draft-command-hatch-pattern-load",
        description: Description {
            message_id: "draft-command-hatch-pattern-load-description",
        },
    },
    Names {
        id: "core.draft.table.create",
        message_id: "draft-command-table-create",
        description: Description {
            message_id: "draft-command-table-create-description",
        },
    },
    Names {
        id: "core.draft.table.modify",
        message_id: "draft-command-table-modify",
        description: Description {
            message_id: "draft-command-table-modify-description",
        },
    },
    Names {
        id: "core.draft.table.cell.set",
        message_id: "draft-command-table-cell-set",
        description: Description {
            message_id: "draft-command-table-cell-set-description",
        },
    },
    Names {
        id: "core.draft.table.row.insert",
        message_id: "draft-command-table-row-insert",
        description: Description {
            message_id: "draft-command-table-row-insert-description",
        },
    },
    Names {
        id: "core.draft.table.row.delete",
        message_id: "draft-command-table-row-delete",
        description: Description {
            message_id: "draft-command-table-row-delete-description",
        },
    },
    Names {
        id: "core.draft.table.column.insert",
        message_id: "draft-command-table-column-insert",
        description: Description {
            message_id: "draft-command-table-column-insert-description",
        },
    },
    Names {
        id: "core.draft.table.column.delete",
        message_id: "draft-command-table-column-delete",
        description: Description {
            message_id: "draft-command-table-column-delete-description",
        },
    },
    Names {
        id: "core.draft.table.merge",
        message_id: "draft-command-table-merge",
        description: Description {
            message_id: "draft-command-table-merge-description",
        },
    },
    Names {
        id: "core.draft.table.unmerge",
        message_id: "draft-command-table-unmerge",
        description: Description {
            message_id: "draft-command-table-unmerge-description",
        },
    },
    Names {
        id: "core.draft.array.create",
        message_id: "draft-command-array-create",
        description: Description {
            message_id: "draft-command-array-create-description",
        },
    },
    Names {
        id: "core.draft.array.modify",
        message_id: "draft-command-array-modify",
        description: Description {
            message_id: "draft-command-array-modify-description",
        },
    },
    Names {
        id: "core.draft.purge",
        message_id: "draft-command-purge",
        description: Description {
            message_id: "draft-command-purge-description",
        },
    },
    Names {
        id: "core.draft.dimension.create",
        message_id: "draft-command-dimension-create",
        description: Description {
            message_id: "draft-command-dimension-create-description",
        },
    },
    Names {
        id: "core.draft.chain.create",
        message_id: "draft-command-chain-create",
        description: Description {
            message_id: "draft-command-chain-create-description",
        },
    },
    Names {
        id: "core.draft.chain.add",
        message_id: "draft-command-chain-add",
        description: Description {
            message_id: "draft-command-chain-add-description",
        },
    },
    Names {
        id: "core.draft.chain.remove",
        message_id: "draft-command-chain-remove",
        description: Description {
            message_id: "draft-command-chain-remove-description",
        },
    },
    Names {
        id: "core.draft.chain.respace",
        message_id: "draft-command-chain-respace",
        description: Description {
            message_id: "draft-command-chain-respace-description",
        },
    },
    Names {
        id: "core.draft.leader.create",
        message_id: "draft-command-leader-create",
        description: Description {
            message_id: "draft-command-leader-create-description",
        },
    },
    Names {
        id: "core.draft.multileader.create",
        message_id: "draft-command-multileader-create",
        description: Description {
            message_id: "draft-command-multileader-create-description",
        },
    },
    Names {
        id: "core.draft.annotation.scales",
        message_id: "draft-command-annotation-scales",
        description: Description {
            message_id: "draft-command-annotation-scales-description",
        },
    },
];

/// The result of a creating command.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Created {
    /// The new object.
    pub id: IdParam,
}

/// Parameters of `core.draft.space.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSpace {
    /// The name.
    #[schemars(length(max = 255))]
    pub name: String,
    /// Model space or layout.
    pub kind: SpaceKind,
    /// The unit; `unitless` for a model space and `millimeter` for a layout when absent.
    #[serde(default)]
    pub unit: Option<SpaceUnit>,
}

/// Parameters of `core.draft.entity.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateEntity {
    /// The space or block definition the entity belongs to.
    pub owner: IdParam,
    /// The geometry: an `arc-geom` curve in its JSON form (`kind` `segment`, `ray`, `line`,
    /// `circle`, `arc`, `ellipse`, `elliptical_arc`, `polyline` or `nurbs`), or
    /// `{"kind": "point", "position": [x, y]}`.
    pub geometry: serde_json::Value,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of the modify commands.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetField {
    /// The objects to change.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// The field name.
    #[schemars(length(max = 64))]
    pub field: String,
    /// The value in canonical JSON form; `null` for absent.
    pub value: serde_json::Value,
}

/// Parameters of the delete commands.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    /// The objects to delete.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
}

/// Parameters of `core.draft.layer.create`; the other layer fields take their defaults.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLayer {
    /// The name, unique among layers.
    #[schemars(length(max = 255))]
    pub name: String,
    /// The linear RGBA color; opaque white when absent.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// The linetype; continuous when absent.
    #[serde(default)]
    pub linetype: Option<IdParam>,
    /// The lineweight in millimeters; the default lineweight when absent.
    #[serde(default)]
    pub lineweight: Option<f64>,
}

/// One element of `core.draft.linetype.load`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ElementParam {
    /// The kind.
    pub kind: ElementKind,
    /// The length in pattern units.
    #[serde(default)]
    pub length: f64,
    /// The text or shape name of a text or shape element.
    #[serde(default)]
    pub content: Option<String>,
    /// The text style or shape file name.
    #[serde(default)]
    pub style: Option<String>,
    /// The size factor of a text or shape element.
    #[serde(default)]
    pub scale: Option<f64>,
    /// The rotation in radians of a text or shape element.
    #[serde(default)]
    pub rotation: Option<f64>,
    /// The offset of a text or shape element.
    #[serde(default)]
    pub offset: Option<[f64; 2]>,
}

/// Parameters of `core.draft.linetype.load`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadLinetype {
    /// The name, unique among linetypes.
    #[schemars(length(max = 255))]
    pub name: String,
    /// The description.
    #[serde(default)]
    pub description: Option<String>,
    /// The pattern.
    #[schemars(length(min = 1, max = 256))]
    pub elements: Vec<ElementParam>,
}

/// One attribute definition of `core.draft.block.define`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttributeParam {
    /// The tag.
    #[schemars(length(max = 255))]
    pub tag: String,
    /// The prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The default value.
    #[serde(default)]
    pub default: Option<String>,
    /// The value is fixed by the definition.
    #[serde(default)]
    pub constant: bool,
    /// The value is not displayed.
    #[serde(default)]
    pub invisible: bool,
    /// The default is taken without asking.
    #[serde(default)]
    pub preset: bool,
    /// The value has several lines.
    #[serde(default)]
    pub multiline: bool,
    /// The position in definition coordinates.
    #[serde(default)]
    pub position: [f64; 2],
}

/// Parameters of `core.draft.block.define`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefineBlock {
    /// The name, unique among block definitions.
    #[schemars(length(max = 255))]
    pub name: String,
    /// The base point.
    #[serde(default)]
    pub base_point: [f64; 2],
    /// The unit; `unitless` when absent.
    #[serde(default)]
    pub unit: Option<SpaceUnit>,
    /// The attribute definitions.
    #[serde(default)]
    #[schemars(length(max = 1024))]
    pub attributes: Vec<AttributeParam>,
    /// Entities moved into the new definition.
    #[serde(default)]
    #[schemars(length(max = 10000))]
    pub objects: Vec<IdParam>,
}

/// Parameters of `core.draft.block.insert`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertBlock {
    /// The space or block definition the reference belongs to.
    pub owner: IdParam,
    /// The block definition.
    pub block: IdParam,
    /// The insertion point.
    pub position: [f64; 2],
    /// The per-axis scale; 1 when absent.
    #[serde(default)]
    pub scale: Option<[f64; 3]>,
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
    /// Attribute values by tag; other attributes take their definition's default.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `core.draft.order.front` and `core.draft.order.back`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Order {
    /// The entities to move, all in one container.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
}

/// Parameters of `core.draft.order.above` and `core.draft.order.below`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderRelative {
    /// The entities to move, all in the container of `reference`.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// The entity to place them next to.
    pub reference: IdParam,
}

/// The result of `core.draft.layout.create`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreatedLayout {
    /// The new layout space.
    pub space: IdParam,
    /// Its layout settings.
    pub layout: IdParam,
}

/// Parameters of `core.draft.layout.create`; the other settings take their defaults.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLayout {
    /// The name of the layout space.
    #[schemars(length(max = 255))]
    pub name: String,
    /// The unit; `millimeter` when absent.
    #[serde(default)]
    pub unit: Option<SpaceUnit>,
    /// The name of the paper size.
    #[serde(default)]
    pub paper_name: Option<String>,
    /// The paper width as named, in the layout's unit.
    pub paper_width: f64,
    /// The paper height as named.
    pub paper_height: f64,
    /// The orientation; `portrait` when absent.
    #[serde(default)]
    pub orientation: Option<Orientation>,
}

/// Parameters of `core.draft.viewport.create`; the other fields take their defaults.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateViewport {
    /// The layout space that owns the viewport.
    pub layout: IdParam,
    /// The space shown.
    pub model: IdParam,
    /// The center on the sheet.
    pub center: [f64; 2],
    /// The width on the sheet.
    pub width: f64,
    /// The height on the sheet.
    pub height: f64,
    /// The model point shown at the center; the origin when absent.
    #[serde(default)]
    pub view_center: [f64; 2],
    /// Paper length per model length in one unit; 1 when absent.
    #[serde(default)]
    pub scale: Option<f64>,
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
}

/// Parameters of `core.draft.wipeout.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWipeout {
    /// The space or block definition the wipeout belongs to.
    pub owner: IdParam,
    /// The boundary: a closed `arc-geom` polyline in its JSON form.
    pub boundary: serde_json::Value,
    /// Whether the boundary is drawn.
    #[serde(default)]
    pub frame: bool,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// The kind of a style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StyleKind {
    /// A text style.
    Text,
    /// A dimension style.
    Dimension,
    /// A leader style.
    Leader,
    /// A table style.
    Table,
}

impl StyleKind {
    /// The type key of the kind.
    #[must_use]
    pub fn type_key(self) -> &'static str {
        match self {
            Self::Text => TEXT_STYLE,
            Self::Dimension => DIM_STYLE,
            Self::Leader => LEADER_STYLE,
            Self::Table => TABLE_STYLE,
        }
    }
}

/// Parameters of `core.draft.style.create`; the other fields take their defaults.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateStyle {
    /// The kind.
    pub kind: StyleKind,
    /// The name, unique among styles of the kind.
    #[schemars(length(max = 255))]
    pub name: String,
}

/// Parameters of `core.draft.style.defaults`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Defaults {}

/// The result of `core.draft.style.defaults`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreatedStyles {
    /// The new styles, in the order text, dimension, leader, table.
    pub created: Vec<IdParam>,
}

/// The horizontal alignment of a single-line text.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HAlignParam {
    /// The position is the left end.
    #[default]
    Left,
    /// The position is the middle.
    Center,
    /// The position is the right end.
    Right,
}

/// The vertical alignment of a single-line text.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VAlignParam {
    /// The position is on the baseline.
    #[default]
    Baseline,
    /// The position is at the bottom of the line box.
    Bottom,
    /// The position is halfway up.
    Middle,
    /// The position is at the cap height.
    Top,
}

/// The attachment point of a rich text.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentParam {
    /// Top left.
    #[default]
    TopLeft,
    /// Top center.
    TopCenter,
    /// Top right.
    TopRight,
    /// Middle left.
    MiddleLeft,
    /// Middle center.
    MiddleCenter,
    /// Middle right.
    MiddleRight,
    /// Bottom left.
    BottomLeft,
    /// Bottom center.
    BottomCenter,
    /// Bottom right.
    BottomRight,
}

/// Parameters of `core.draft.text.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateText {
    /// The space or block definition the text belongs to.
    pub owner: IdParam,
    /// The text: one line, no control characters except tab.
    #[schemars(length(max = 4096))]
    pub content: String,
    /// The position.
    pub position: [f64; 2],
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The text style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The height; absent or 0 takes the style's.
    #[serde(default)]
    pub height: Option<f64>,
    /// The horizontal alignment.
    #[serde(default)]
    pub halign: HAlignParam,
    /// The vertical alignment.
    #[serde(default)]
    pub valign: VAlignParam,
    /// Fields inserted into `content` (CORE-DRAFT-007).
    #[serde(default)]
    pub fields: Vec<crate::fields::Field>,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `core.draft.rich_text.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRichText {
    /// The space or block definition the text belongs to.
    pub owner: IdParam,
    /// The paragraphs; give these or `text`.
    #[serde(default)]
    pub paragraphs: Option<Vec<Paragraph>>,
    /// Plain text: each line becomes a paragraph.
    #[serde(default)]
    pub text: Option<String>,
    /// The attachment point.
    pub position: [f64; 2],
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The text style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The height; absent or 0 takes the style's.
    #[serde(default)]
    pub height: Option<f64>,
    /// The wrapping width; absent or 0 does not wrap.
    #[serde(default)]
    pub width: Option<f64>,
    /// Which point of the text block `position` is.
    #[serde(default)]
    pub attachment: AttachmentParam,
    /// The number of columns, 1 to 64.
    #[serde(default)]
    pub columns: Option<i64>,
    /// The space between columns.
    #[serde(default)]
    pub column_gutter: Option<f64>,
    /// The height of a column; 0 keeps one column.
    #[serde(default)]
    pub column_height: Option<f64>,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `core.draft.rich_text.set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRichText {
    /// The rich texts to change.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// The new paragraphs; give these or `text`.
    #[serde(default)]
    pub paragraphs: Option<Vec<Paragraph>>,
    /// Plain text: each line becomes a paragraph.
    #[serde(default)]
    pub text: Option<String>,
}

/// Parameters of `core.draft.font.substitute`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Substitute {
    /// The font name to substitute.
    #[schemars(length(max = 255))]
    pub requested: String,
    /// The font used in its place; absent removes the entry.
    #[serde(default)]
    #[schemars(length(max = 255))]
    pub substitute: Option<String>,
}

pub(crate) fn wrong_type(id: ObjectId) -> Error {
    Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", id.to_string())
}

pub(crate) fn field_invalid(field: &str) -> Error {
    Error::new(codes::FIELD_INVALID).with_arg("field", field)
}

/// The type key of the live object `id`.
fn key_of(snapshot: &Snapshot, id: ObjectId) -> Result<String> {
    let handle = snapshot.resolve(id)?;
    Ok(snapshot
        .type_key(handle)
        .map(|k| k.as_str().to_owned())
        .unwrap_or_default())
}

/// Fails with `draft.object.wrong_type` unless `id` is live and of one of `keys`.
pub(crate) fn expect(snapshot: &Snapshot, id: ObjectId, keys: &[&str]) -> Result<String> {
    let key = key_of(snapshot, id)?;
    if keys.contains(&key.as_str()) {
        Ok(key)
    } else {
        Err(wrong_type(id))
    }
}

pub(crate) fn values_of(snapshot: &Snapshot, id: ObjectId) -> Result<Vec<Option<Value>>> {
    Ok(snapshot.get(snapshot.resolve(id)?)?.values.clone())
}

/// The entities of `container` with their draw-order keys.
fn members(snapshot: &Snapshot, container: ObjectId) -> Vec<(ObjectId, i64)> {
    snapshot
        .referrers(container)
        .filter(|r| r.field == ReferenceField::Owner)
        .filter_map(|r| {
            let handle = snapshot.resolve(r.source).ok()?;
            let key = snapshot.type_key(handle)?;
            if !ENTITIES.contains(&key.as_str()) {
                return None;
            }
            match snapshot.get(handle).ok()?.values.get(entity::DRAW_ORDER) {
                Some(Some(Value::I64(k))) => Some((r.source, *k)),
                _ => Some((r.source, 0)),
            }
        })
        .collect()
}

fn set_order(ctx: &mut Context<'_>, changes: Vec<(ObjectId, i64)>) -> Result<()> {
    let txn = ctx.txn()?;
    for (id, key) in changes {
        txn.modify(id, FieldPath::new("draw_order"), Some(Value::I64(key)))?;
    }
    Ok(())
}

pub(crate) fn point2([x, y]: [f64; 2]) -> Value {
    Value::Point2(Point2::new(x, y))
}

pub(crate) fn color([r, g, b, a]: [f32; 4]) -> Value {
    Value::Color(Box::new(ColorValue {
        rgba: Color::new(r, g, b, a),
        source: None,
    }))
}

pub(crate) fn set(values: &mut [Option<Value>], index: usize, value: Option<Value>) {
    if let Some(slot) = values.get_mut(index) {
        *slot = value;
    }
}

/// Creates an object of `key` owned by `owner` from `values` and selects it.
pub(crate) fn create(
    ctx: &mut Context<'_>,
    key: &str,
    owner: Option<ObjectId>,
    values: Vec<Option<Value>>,
) -> Result<ObjectId> {
    let id: ObjectId = ctx.new_id();
    let header = Header {
        owner,
        ..Header::default()
    };
    ctx.txn()?.create(key, id, header, values)?;
    ctx.set_selection_after(vec![id]);
    Ok(id)
}

/// Creates the entity of `key` in `owner` on top of the container's draw order.
pub(crate) fn create_entity(
    ctx: &mut Context<'_>,
    key: &str,
    owner: ObjectId,
    layer: Option<IdParam>,
    kind: Vec<(usize, Value)>,
) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, owner, CONTAINERS)?;
    let mut values = types::defaults(snapshot.registry(), key);
    set(
        &mut values,
        entity::LAYER,
        layer.map(|IdParam(l)| Value::Reference(l)),
    );
    for (index, value) in kind {
        set(&mut values, index, Some(value));
    }
    // Placed with a provisional identifier, then created on top.
    let mut current = members(&snapshot, owner);
    let provisional = ObjectId::from_u128(0);
    current.push((provisional, 0));
    let mut changes = reorder(&current, &[provisional], Place::Front)?;
    let key_of_new = changes
        .iter()
        .find(|(i, _)| *i == provisional)
        .map_or(0, |(_, k)| *k);
    changes.retain(|(i, _)| *i != provisional);
    set(
        &mut values,
        entity::DRAW_ORDER,
        Some(Value::I64(key_of_new)),
    );
    let id = create(ctx, key, Some(owner), values)?;
    set_order(ctx, changes)?;
    Ok(Created { id: IdParam(id) })
}

fn create_entity_command(ctx: &mut Context<'_>, p: CreateEntity) -> Result<Created> {
    let invalid = || field_invalid("geometry");
    if p.geometry.get("kind").and_then(serde_json::Value::as_str) == Some("point") {
        let position: [f64; 2] = p
            .geometry
            .get("position")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(invalid)?;
        return create_entity(
            ctx,
            types::POINT,
            p.owner.0,
            p.layer,
            vec![(entity::KIND, point2(position))],
        );
    }
    let curve: Curve2 = serde_json::from_value(p.geometry).map_err(|_| invalid())?;
    let key = match &curve {
        Curve2::Segment(_) => types::LINE,
        Curve2::Ray(_) => types::RAY,
        Curve2::Line(_) => types::CONSTRUCTION_LINE,
        Curve2::Circle(_) => types::CIRCLE,
        Curve2::Arc(_) => types::ARC,
        Curve2::Ellipse(_) => types::ELLIPSE,
        Curve2::EllipticalArc(_) => types::ELLIPTICAL_ARC,
        Curve2::Polyline(_) => types::POLYLINE,
        Curve2::Nurbs(curve) => {
            let values = crate::spline::values(curve, &[]);
            return create_entity(ctx, types::SPLINE, p.owner.0, p.layer, values);
        }
    };
    let bytes = Value::Geometry(GeometryBytes(curve.encode()));
    create_entity(ctx, key, p.owner.0, p.layer, vec![(entity::KIND, bytes)])
}

fn numbers<const N: usize>(json: &serde_json::Value) -> Option<[f64; N]> {
    let list: Vec<f64> = json
        .as_array()?
        .iter()
        .map(serde_json::Value::as_f64)
        .collect::<Option<_>>()?;
    list.try_into().ok()
}

/// The value of a field of type `value_type` from its canonical JSON form: as `arc-doc`
/// serializes values, except that an enum also accepts its variant name, a color also accepts
/// `[r, g, b, a]`, and a geometry is an `arc-geom` curve in its JSON form. Lists, maps and
/// records nest at most 16 levels. `None` when the JSON does not fit.
#[must_use]
pub fn field_value(value_type: &ValueType, json: &serde_json::Value) -> Option<Value> {
    from_json(value_type, json, 0)
}

fn from_json(value_type: &ValueType, json: &serde_json::Value, depth: u32) -> Option<Value> {
    if depth > MAX_JSON_DEPTH {
        return None;
    }
    Some(match value_type {
        ValueType::Bool => Value::Bool(json.as_bool()?),
        ValueType::I64 => Value::I64(json.as_i64()?),
        ValueType::F64 => Value::F64(json.as_f64()?),
        ValueType::String => Value::String(json.as_str()?.to_owned()),
        ValueType::Enum(variants) => Value::Enum(match json.as_str() {
            Some(name) => variants.iter().find(|v| v.name == name)?.code,
            None => u32::try_from(json.as_u64()?).ok()?,
        }),
        ValueType::Point2 => {
            let [x, y] = numbers(json)?;
            Value::Point2(Point2::new(x, y))
        }
        ValueType::Vector3 => {
            let [x, y, z] = numbers(json)?;
            Value::Vector3(Vector3::new(x, y, z))
        }
        ValueType::Color => {
            let (rgba, source) = match json.get("rgba") {
                Some(rgba) => (rgba, json.get("source").cloned()),
                None => (json, None),
            };
            let [r, g, b, a] = numbers::<4>(rgba)?;
            let narrow = |x: f64| {
                #[expect(clippy::cast_possible_truncation, reason = "colors are stored as f32")]
                let y = x as f32;
                y
            };
            let source: Option<ColorSource> = match source {
                None | Some(serde_json::Value::Null) => None,
                Some(s) => Some(serde_json::from_value(s).ok()?),
            };
            Value::Color(Box::new(ColorValue {
                rgba: Color::new(narrow(r), narrow(g), narrow(b), narrow(a)),
                source,
            }))
        }
        ValueType::Reference(_) => Value::Reference(json.as_str()?.parse().ok()?),
        ValueType::Geometry(_) => {
            let curve: Curve2 = serde_json::from_value(json.clone()).ok()?;
            Value::Geometry(GeometryBytes(curve.encode()))
        }
        ValueType::List(element) => Value::List(
            json.as_array()?
                .iter()
                .map(|item| from_json(element, item, depth + 1))
                .collect::<Option<_>>()?,
        ),
        ValueType::Map(element) => Value::Map(
            json.as_object()?
                .iter()
                .map(|(k, v)| Some((k.clone(), from_json(element, v, depth + 1)?)))
                .collect::<Option<_>>()?,
        ),
        ValueType::Record(fields) => {
            let items = json.as_array()?;
            if items.len() != fields.len() {
                return None;
            }
            Value::Record(
                fields
                    .iter()
                    .zip(items)
                    .map(|(field, item)| match item {
                        serde_json::Value::Null => Some(None),
                        _ => from_json(&field.value_type, item, depth + 1).map(Some),
                    })
                    .collect::<Option<_>>()?,
            )
        }
        _ => return None,
    })
}

/// Sets `field` of every listed object, each of one of `family`.
fn modify(ctx: &mut Context<'_>, p: SetField, family: &[&str]) -> Result<()> {
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        let snapshot = txn.snapshot();
        let key = expect(snapshot, id, family)?;
        let value_type = snapshot
            .registry()
            .get(&key)
            .and_then(|decl| decl.schema.fields.iter().find(|f| f.name == p.field))
            .map(|f| f.value_type.clone())
            .ok_or_else(|| field_invalid(&p.field))?;
        let value = match &p.value {
            serde_json::Value::Null => None,
            json => Some(field_value(&value_type, json).ok_or_else(|| field_invalid(&p.field))?),
        };
        txn.modify(id, FieldPath::new(p.field.as_str()), value)?;
    }
    Ok(())
}

pub(crate) fn delete(ctx: &mut Context<'_>, p: Delete, family: &[&str]) -> Result<()> {
    let limits = *ctx.limits();
    let txn = ctx.txn()?;
    let snapshot = txn.snapshot().clone();
    // Dimensions keep the points of geometry about to be deleted as their last positions.
    let doomed: Vec<ObjectId> = p.objects.iter().map(|IdParam(id)| *id).collect();
    let mut dimensions: Vec<ObjectId> = doomed
        .iter()
        .flat_map(|id| {
            snapshot
                .referrers(*id)
                .map(|r| r.source)
                .collect::<Vec<_>>()
        })
        .filter(|id| !doomed.contains(id) && key_of(&snapshot, *id).is_ok_and(|k| k == DIMENSION))
        .collect();
    dimensions.sort();
    dimensions.dedup();
    if !dimensions.is_empty() {
        let options = crate::dimension::resolution_options();
        for id in dimensions {
            if let Some(points) = crate::dimension::frozen_points(&snapshot, id, &options) {
                txn.modify(id, FieldPath::new("points"), Some(points))?;
            }
        }
    }
    // Hatches keep the boundary they last followed.
    let mut hatches: Vec<ObjectId> = doomed
        .iter()
        .flat_map(|id| {
            snapshot
                .referrers(*id)
                .map(|r| r.source)
                .collect::<Vec<_>>()
        })
        .filter(|id| {
            !doomed.contains(id) && key_of(&snapshot, *id).is_ok_and(|k| k == crate::hatch::HATCH)
        })
        .collect();
    hatches.sort();
    hatches.dedup();
    if !hatches.is_empty() {
        let options = crate::dimension::resolution_options();
        for id in hatches {
            if let Some(loops) = crate::hatch::frozen_loops(&snapshot, id, &options) {
                txn.modify(id, FieldPath::new("loops"), Some(loops))?;
            }
        }
    }
    for IdParam(id) in p.objects {
        expect(txn.snapshot(), id, family)?;
        txn.delete(id)?;
    }
    txn.apply_delete_rules(&limits)?;
    Ok(())
}

fn create_space(ctx: &mut Context<'_>, p: CreateSpace) -> Result<Created> {
    let unit = p.unit.unwrap_or(match p.kind {
        SpaceKind::Model => SpaceUnit::Unitless,
        SpaceKind::Layout => SpaceUnit::Millimeter,
    });
    let values = vec![
        Some(Value::String(p.name)),
        Some(Value::Enum(p.kind as u32)),
        Some(Value::Enum(unit.code())),
    ];
    Ok(Created {
        id: IdParam(create(ctx, SPACE, None, values)?),
    })
}

fn create_layer(ctx: &mut Context<'_>, p: CreateLayer) -> Result<Created> {
    let mut values = types::defaults(ctx.snapshot().registry(), LAYER);
    set(&mut values, layer::NAME, Some(Value::String(p.name)));
    if let Some(c) = p.color {
        set(&mut values, layer::COLOR, Some(color(c)));
    }
    set(
        &mut values,
        layer::LINETYPE,
        p.linetype.map(|IdParam(l)| Value::Reference(l)),
    );
    set(&mut values, layer::LINEWEIGHT, p.lineweight.map(Value::F64));
    Ok(Created {
        id: IdParam(create(ctx, LAYER, None, values)?),
    })
}

fn load_linetype(ctx: &mut Context<'_>, p: LoadLinetype) -> Result<Created> {
    let elements = p
        .elements
        .into_iter()
        .map(|e| {
            Value::Record(vec![
                Some(Value::Enum(e.kind as u32)),
                Some(Value::F64(e.length)),
                e.content.map(Value::String),
                e.style.map(Value::String),
                Some(Value::F64(e.scale.unwrap_or(1.0))),
                Some(Value::F64(e.rotation.unwrap_or(0.0))),
                e.offset.map(point2),
            ])
        })
        .collect();
    let values = vec![
        Some(Value::String(p.name)),
        p.description.map(Value::String),
        Some(Value::List(elements)),
    ];
    Ok(Created {
        id: IdParam(create(ctx, LINETYPE, None, values)?),
    })
}

fn define_block(ctx: &mut Context<'_>, p: DefineBlock) -> Result<Created> {
    let attributes = p
        .attributes
        .into_iter()
        .map(|a| {
            Value::Record(vec![
                Some(Value::String(a.tag)),
                a.prompt.map(Value::String),
                a.default.map(Value::String),
                Some(Value::Bool(a.constant)),
                Some(Value::Bool(a.invisible)),
                Some(Value::Bool(a.preset)),
                Some(Value::Bool(a.multiline)),
                Some(point2(a.position)),
            ])
        })
        .collect();
    let values = vec![
        Some(Value::String(p.name)),
        Some(point2(p.base_point)),
        Some(Value::Enum(p.unit.unwrap_or(SpaceUnit::Unitless).code())),
        None,
        Some(Value::List(attributes)),
    ];
    let block = create(ctx, BLOCK, None, values)?;
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        expect(txn.snapshot(), id, ENTITIES)?;
        txn.relink(id, Some(block))?;
    }
    Ok(Created { id: IdParam(block) })
}

fn insert_block(ctx: &mut Context<'_>, p: InsertBlock) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.block.0, &[BLOCK])?;
    let definition = values_of(&snapshot, p.block.0)?;
    let mut attributes = BTreeMap::new();
    if let Some(Some(Value::List(list))) = definition.get(table::BLOCK_ATTRIBUTES) {
        for attribute in list {
            if let Value::Record(fields) = attribute
                && let Some(tag) = types::text_at(fields, 0)
            {
                let default = types::text_at(fields, 2).unwrap_or_default();
                attributes.insert(tag.to_owned(), Value::String(default.to_owned()));
            }
        }
    }
    for (tag, value) in p.attributes {
        if !attributes.contains_key(&tag) {
            return Err(Error::new(codes::BLOCK_ATTRIBUTE_UNKNOWN).with_arg("tag", tag));
        }
        attributes.insert(tag, Value::String(value));
    }
    let [sx, sy, sz] = p.scale.unwrap_or([1.0; 3]);
    let kind = vec![
        (entity::KIND, Value::Reference(p.block.0)),
        (entity::REF_POSITION, point2(p.position)),
        (entity::REF_SCALE, Value::Vector3(Vector3::new(sx, sy, sz))),
        (entity::REF_ROTATION, Value::F64(p.rotation)),
        (entity::REF_ATTRIBUTES, Value::Map(attributes)),
    ];
    create_entity(ctx, BLOCK_REF, p.owner.0, p.layer, kind)
}

fn create_layout(ctx: &mut Context<'_>, p: CreateLayout) -> Result<CreatedLayout> {
    let unit = p.unit.unwrap_or(SpaceUnit::Millimeter);
    let space = create(
        ctx,
        SPACE,
        None,
        vec![
            Some(Value::String(p.name)),
            Some(Value::Enum(SpaceKind::Layout as u32)),
            Some(Value::Enum(unit.code())),
        ],
    )?;
    let mut values = types::defaults(ctx.snapshot().registry(), LAYOUT);
    set(
        &mut values,
        crate::layout::settings::PAPER_NAME,
        p.paper_name.map(Value::String),
    );
    set(
        &mut values,
        crate::layout::settings::PAPER_WIDTH,
        Some(Value::F64(p.paper_width)),
    );
    set(
        &mut values,
        crate::layout::settings::PAPER_HEIGHT,
        Some(Value::F64(p.paper_height)),
    );
    if let Some(o) = p.orientation {
        set(
            &mut values,
            crate::layout::settings::ORIENTATION,
            Some(Value::Enum(o as u32)),
        );
    }
    let layout = create(ctx, LAYOUT, Some(space), values)?;
    ctx.set_selection_after(vec![space]);
    Ok(CreatedLayout {
        space: IdParam(space),
        layout: IdParam(layout),
    })
}

fn create_viewport(ctx: &mut Context<'_>, p: CreateViewport) -> Result<Created> {
    let mut values = types::defaults(ctx.snapshot().registry(), VIEWPORT);
    set(
        &mut values,
        viewport::MODEL,
        Some(Value::Reference(p.model.0)),
    );
    set(&mut values, viewport::CENTER, Some(point2(p.center)));
    set(&mut values, viewport::WIDTH, Some(Value::F64(p.width)));
    set(&mut values, viewport::HEIGHT, Some(Value::F64(p.height)));
    set(
        &mut values,
        viewport::VIEW_CENTER,
        Some(point2(p.view_center)),
    );
    set(
        &mut values,
        viewport::SCALE,
        Some(Value::F64(p.scale.unwrap_or(1.0))),
    );
    set(
        &mut values,
        viewport::ROTATION,
        Some(Value::F64(p.rotation)),
    );
    Ok(Created {
        id: IdParam(create(ctx, VIEWPORT, Some(p.layout.0), values)?),
    })
}

/// `core.draft.viewport.modify`: the view fields of a locked viewport are refused.
fn modify_viewport(ctx: &mut Context<'_>, p: SetField) -> Result<()> {
    if viewport::VIEW_FIELDS.contains(&p.field.as_str()) {
        let snapshot = ctx.snapshot().clone();
        for IdParam(id) in &p.objects {
            expect(&snapshot, *id, &[VIEWPORT])?;
            if values_of(&snapshot, *id)?.get(viewport::LOCKED) == Some(&Some(Value::Bool(true))) {
                return Err(Error::new(codes::VIEWPORT_LOCKED).with_detail("id", id.to_string()));
            }
        }
    }
    modify(ctx, p, &[VIEWPORT])
}

fn create_wipeout(ctx: &mut Context<'_>, p: CreateWipeout) -> Result<Created> {
    let invalid = || field_invalid("boundary");
    let curve: Curve2 = serde_json::from_value(p.boundary).map_err(|_| invalid())?;
    if !matches!(&curve, Curve2::Polyline(_)) {
        return Err(invalid());
    }
    let kind = vec![
        (entity::KIND, Value::Geometry(GeometryBytes(curve.encode()))),
        (entity::WIPEOUT_FRAME, Value::Bool(p.frame)),
    ];
    create_entity(ctx, types::WIPEOUT, p.owner.0, p.layer, kind)
}

fn create_style(ctx: &mut Context<'_>, kind: StyleKind, name: String) -> Result<ObjectId> {
    let key = kind.type_key();
    let mut values = types::defaults(ctx.snapshot().registry(), key);
    set(&mut values, table::NAME, Some(Value::String(name)));
    create(ctx, key, None, values)
}

/// Creates a style named [`DEFAULT_NAME`] of every kind that has none.
fn create_defaults(ctx: &mut Context<'_>, _: Defaults) -> Result<CreatedStyles> {
    let mut created = Vec::new();
    for kind in [
        StyleKind::Text,
        StyleKind::Dimension,
        StyleKind::Leader,
        StyleKind::Table,
    ] {
        let exists = ctx
            .txn()?
            .snapshot()
            .iter_type(kind.type_key())
            .is_ok_and(|mut all| {
                all.any(|(_, o)| types::text_at(&o.values, table::NAME) == Some(DEFAULT_NAME))
            });
        if !exists {
            created.push(IdParam(create_style(ctx, kind, DEFAULT_NAME.to_owned())?));
        }
    }
    ctx.set_selection_after(created.iter().map(|IdParam(id)| *id).collect());
    Ok(CreatedStyles { created })
}

/// `core.draft.purge`: deletes the objects, each unused in the transaction's state.
fn purge(ctx: &mut Context<'_>, p: Delete) -> Result<()> {
    let keys = crate::purge::purgeable();
    let limits = *ctx.limits();
    let txn = ctx.txn()?;
    for IdParam(id) in &p.objects {
        expect(txn.snapshot(), *id, &keys)?;
        if crate::purge::in_use(txn.snapshot(), *id) {
            return Err(Error::new(codes::PURGE_IN_USE).with_detail("id", id.to_string()));
        }
    }
    for IdParam(id) in p.objects {
        // An object listed twice is deleted once.
        if txn.snapshot().resolve(id).is_ok() {
            txn.delete(id)?;
        }
    }
    txn.apply_delete_rules(&limits)?;
    Ok(())
}

/// Moves `objects`, which must share one container, to `place`.
fn order(ctx: &mut Context<'_>, objects: Vec<IdParam>, place: Place) -> Result<()> {
    let snapshot = ctx.txn()?.snapshot().clone();
    let mut container = None;
    let anchor = match place {
        Place::Above(r) | Place::Below(r) => Some(r),
        Place::Front | Place::Back => None,
    };
    let ids: Vec<ObjectId> = objects.into_iter().map(|IdParam(id)| id).collect();
    for id in ids.iter().chain(anchor.as_ref()) {
        expect(&snapshot, *id, ENTITIES)?;
        let owner = snapshot.get(snapshot.resolve(*id)?)?.header.owner;
        if container.is_some_and(|c| Some(c) != owner) {
            return Err(
                Error::new(codes::ORDER_CONTAINER_MISMATCH).with_detail("id", id.to_string())
            );
        }
        container = Some(owner.ok_or_else(|| wrong_type(*id))?);
    }
    let Some(container) = container else {
        return Ok(());
    };
    let changes = reorder(&members(&snapshot, container), &ids, place)?;
    set_order(ctx, changes)
}

/// The paragraphs of `paragraphs` or `text` (exactly one), checked against `limits`.
fn paragraphs(
    paragraphs: Option<Vec<Paragraph>>,
    text: Option<String>,
    limits: &arc_base::Limits,
) -> Result<Vec<Paragraph>> {
    match (paragraphs, text) {
        (Some(p), None) => {
            validate_paragraphs(&p, limits)?;
            Ok(p)
        }
        (None, Some(t)) => paragraphs_from_plain(&t, limits),
        (None, None) => Ok(Vec::new()),
        (Some(_), Some(_)) => Err(field_invalid("text")),
    }
}

fn create_text(ctx: &mut Context<'_>, p: CreateText) -> Result<Created> {
    crate::fields::validate(&p.content, &p.fields, ctx.limits())?;
    let mut kind = vec![
        (text_field::FIELDS, crate::fields::list_value(&p.fields)),
        (text_field::CONTENT, Value::String(p.content)),
        (text_field::POSITION, point2(p.position)),
        (text_field::ROTATION, Value::F64(p.rotation)),
        (text_field::HEIGHT, Value::F64(p.height.unwrap_or(0.0))),
        (text_field::HALIGN, Value::Enum(p.halign as u32)),
        (text_field::VALIGN, Value::Enum(p.valign as u32)),
    ];
    kind.extend(
        p.style
            .map(|IdParam(s)| (text_field::STYLE, Value::Reference(s))),
    );
    create_entity(ctx, TEXT, p.owner.0, p.layer, kind)
}

fn create_rich_text(ctx: &mut Context<'_>, p: CreateRichText) -> Result<Created> {
    let limits = *ctx.limits();
    let list = paragraphs(p.paragraphs, p.text, &limits)?;
    let mut kind = vec![
        (text_field::CONTENT, paragraphs_value(&list)),
        (text_field::POSITION, point2(p.position)),
        (text_field::ROTATION, Value::F64(p.rotation)),
        (text_field::HEIGHT, Value::F64(p.height.unwrap_or(0.0))),
        (text_field::WIDTH, Value::F64(p.width.unwrap_or(0.0))),
        (text_field::ATTACHMENT, Value::Enum(p.attachment as u32)),
        (text_field::COLUMNS, Value::I64(p.columns.unwrap_or(1))),
        (
            text_field::COLUMN_GUTTER,
            Value::F64(p.column_gutter.unwrap_or(0.0)),
        ),
        (
            text_field::COLUMN_HEIGHT,
            Value::F64(p.column_height.unwrap_or(0.0)),
        ),
    ];
    kind.extend(
        p.style
            .map(|IdParam(s)| (text_field::STYLE, Value::Reference(s))),
    );
    create_entity(ctx, RICH_TEXT, p.owner.0, p.layer, kind)
}

fn set_rich_text(ctx: &mut Context<'_>, p: SetRichText) -> Result<()> {
    let limits = *ctx.limits();
    let value = paragraphs_value(&paragraphs(p.paragraphs, p.text, &limits)?);
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        expect(txn.snapshot(), id, &[RICH_TEXT])?;
        txn.modify(id, FieldPath::new("paragraphs"), Some(value.clone()))?;
    }
    Ok(())
}

/// Sets or removes one entry of the document's substitution table, creating the table on
/// first use.
fn substitute(ctx: &mut Context<'_>, p: Substitute) -> Result<()> {
    if p.requested.trim().is_empty() || p.substitute.as_deref().is_some_and(|s| s.trim().is_empty())
    {
        return Err(field_invalid("requested"));
    }
    let snapshot = ctx.txn()?.snapshot().clone();
    let mut table = document_table(&snapshot);
    table.set(&p.requested, p.substitute.as_deref());
    let value = table.to_value();
    let id = font_table_id();
    let txn = ctx.txn()?;
    if snapshot.resolve(id).is_ok() {
        expect(&snapshot, id, &[FONT_TABLE])?;
        txn.modify(id, FieldPath::new("entries"), Some(value))?;
    } else {
        txn.create(FONT_TABLE, id, Header::default(), vec![Some(value)])?;
    }
    Ok(())
}

pub(crate) fn spec<P, R>(
    id: &'static str,
    run: impl Fn(&mut Context<'_>, P) -> Result<R> + Send + Sync + 'static,
) -> CommandSpec
where
    P: serde::de::DeserializeOwned + JsonSchema,
    R: Serialize + JsonSchema,
{
    let names = NAMES.iter().find(|n| n.id == id);
    let label = names.map_or(id, |n| n.message_id);
    let description = names.map_or(id, |n| n.description.message_id);
    CommandSpec::new(id, label, description, run)
        .with_undo(UndoBehavior::Undoable)
        .with_permissions([Permission::Edit])
}

pub(crate) fn family(id: &'static str, keys: &'static [&'static str]) -> CommandSpec {
    spec(id, move |ctx, p: SetField| modify(ctx, p, keys))
}

/// Registers every drafting command into `registry`.
///
/// # Errors
///
/// `cmd.duplicate` when one of the identifiers is already registered.
pub fn register_commands(registry: &mut Registry) -> Result<()> {
    let specs = [
        spec("core.draft.space.create", create_space).with_example(
            serde_json::json!({"name": "model", "kind": "model", "unit": "millimeter"}),
        ),
        family("core.draft.space.modify", &[SPACE]),
        spec("core.draft.entity.create", create_entity_command),
        family("core.draft.entity.modify", ENTITIES),
        spec("core.draft.entity.delete", |ctx, p: Delete| {
            delete(ctx, p, ENTITIES)
        }),
        spec("core.draft.layer.create", create_layer)
            .with_example(serde_json::json!({"name": "walls"})),
        family("core.draft.layer.modify", &[LAYER]),
        spec("core.draft.layer.delete", |ctx, p: Delete| {
            delete(ctx, p, &[LAYER])
        }),
        spec("core.draft.linetype.load", load_linetype).with_example(serde_json::json!({
            "name": "dashed",
            "elements": [{"kind": "dash", "length": 0.5}, {"kind": "gap", "length": 0.25}]
        })),
        family("core.draft.linetype.modify", &[LINETYPE]),
        spec("core.draft.block.define", define_block)
            .with_example(serde_json::json!({"name": "door"})),
        family("core.draft.block.modify", &[BLOCK]),
        spec("core.draft.block.insert", insert_block),
        crate::external_commands::create_spec(),
        crate::external_commands::set_clip_spec(),
        spec("core.draft.order.front", |ctx, p: Order| {
            order(ctx, p.objects, Place::Front)
        }),
        spec("core.draft.order.back", |ctx, p: Order| {
            order(ctx, p.objects, Place::Back)
        }),
        spec("core.draft.order.above", |ctx, p: OrderRelative| {
            order(ctx, p.objects, Place::Above(p.reference.0))
        }),
        spec("core.draft.order.below", |ctx, p: OrderRelative| {
            order(ctx, p.objects, Place::Below(p.reference.0))
        }),
        spec("core.draft.layout.create", create_layout).with_example(serde_json::json!({
            "name": "sheet 1", "paper_width": 297.0, "paper_height": 210.0
        })),
        family("core.draft.layout.modify", &[LAYOUT]),
        spec("core.draft.viewport.create", create_viewport),
        spec("core.draft.viewport.modify", modify_viewport),
        spec("core.draft.viewport.delete", |ctx, p: Delete| {
            delete(ctx, p, &[VIEWPORT])
        }),
        spec("core.draft.wipeout.create", create_wipeout),
        spec("core.draft.style.create", |ctx, p: CreateStyle| {
            Ok(Created {
                id: IdParam(create_style(ctx, p.kind, p.name)?),
            })
        })
        .with_example(serde_json::json!({"kind": "text", "name": "notes"})),
        family("core.draft.style.modify", STYLES),
        spec("core.draft.style.delete", |ctx, p: Delete| {
            delete(ctx, p, STYLES)
        }),
        spec("core.draft.style.defaults", create_defaults).with_example(serde_json::json!({})),
        spec("core.draft.purge", purge),
        spec("core.draft.text.create", create_text),
        spec("core.draft.rich_text.create", create_rich_text),
        spec("core.draft.rich_text.set", set_rich_text),
        spec("core.draft.font.substitute", substitute).with_example(
            serde_json::json!({"requested": "Old Drafting", "substitute": "Arc Stroke"}),
        ),
    ];
    for spec in specs
        .into_iter()
        .chain(crate::annotation_commands::specs())
        .chain(crate::spline::specs())
        .chain(crate::plot_style::specs())
        .chain(crate::hatch_commands::specs())
        .chain(crate::table_commands::specs())
        .chain(crate::array::specs())
    {
        registry.register(spec)?;
    }
    Ok(())
}
