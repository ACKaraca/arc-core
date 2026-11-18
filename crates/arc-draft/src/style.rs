//! Text, dimension, leader and table styles (CORE-DRAFT-019).
//!
//! Styles are named document objects holding the data their users read; laying out text or
//! dimensions is a later part. An object uses a style through a reference field made by
//! [`style_reference`]: associative, refusing deletion of a style in use (`block`). Each style
//! has a derived node [`EFFECTIVE`] in the dependency graph: its own field values and the
//! effective data of every style it references. An evaluator of a style user reads that node,
//! so changing a style marks the derived data of every user stale and recomputes it on demand.
//! No style is created implicitly: `core.draft.style.defaults` creates one named `default` of
//! each kind that has none.

use std::sync::Arc;

use arc_base::{ObjectId, Result};
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{Bound, FieldDecl, OnDelete, Strength, TypeKey, TypeValidator, Value, ValueType};

use crate::types::{NAME_BYTES, TEXT_BYTES, flag, name_field, real, reference, text, variants};

/// A text style: font by name, height, width factor, oblique angle and direction flags.
pub const TEXT_STYLE: &str = "core.draft.text_style";
/// A dimension style, optionally the child of a style for all kinds.
pub const DIM_STYLE: &str = "core.draft.dim_style";
/// A leader style.
pub const LEADER_STYLE: &str = "core.draft.leader_style";
/// A table style with title, header and data row formats.
pub const TABLE_STYLE: &str = "core.draft.table_style";

/// Every style type key.
pub const STYLES: &[&str] = &[TEXT_STYLE, DIM_STYLE, LEADER_STYLE, TABLE_STYLE];

/// The derived key of a style's effective data.
pub const EFFECTIVE: &str = "core.draft.style.effective";

/// The largest oblique angle of a text style in radians (85 degrees).
pub const MAX_OBLIQUE: f64 = 85.0 * std::f64::consts::PI / 180.0;

/// The name of the styles `core.draft.style.defaults` creates.
pub const DEFAULT_NAME: &str = "default";

/// Field indices of a dimension style.
pub mod dim {
    /// `parent`: the style for all kinds this one refines.
    pub const PARENT: usize = 1;
    /// `kind`: the dimension kind the style applies to; `all` for a top-level style.
    pub const KIND: usize = 2;
}

/// The reference field type with which an object uses a style of `target`: associative, and
/// a style in use cannot be deleted. `optional` allows the field to be absent.
#[must_use]
pub fn style_reference(target: &str, optional: bool) -> ValueType {
    reference(target, Strength::Associative, OnDelete::Block, optional)
}

fn style_field(name: &str, target: &str) -> FieldDecl {
    FieldDecl::new(name, style_reference(target, true)).optional()
}

fn size(name: &str, default: f64) -> FieldDecl {
    real(name, default, 0.0, f64::MAX)
}

fn arrow() -> FieldDecl {
    FieldDecl::new(
        "arrow",
        variants(&["closed_filled", "closed", "open", "dot", "tick", "none"]),
    )
    .with_default(Value::Enum(0))
}

fn text_style_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        text("font", NAME_BYTES),
        size("height", 0.0),
        real("width_factor", 1.0, f64::MIN_POSITIVE, f64::MAX),
        real("oblique", 0.0, -MAX_OBLIQUE, MAX_OBLIQUE),
        flag("backwards", false),
        flag("upside_down", false),
        flag("vertical", false),
        text("description", TEXT_BYTES),
    ]
}

fn dim_style_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        style_field("parent", DIM_STYLE),
        FieldDecl::new(
            "kind",
            variants(&[
                "all",
                "linear",
                "angular",
                "radial",
                "diameter",
                "ordinate",
                "arc_length",
            ]),
        )
        .with_default(Value::Enum(0)),
        style_field("text_style", TEXT_STYLE),
        size("text_height", 2.5),
        size("text_gap", 0.625),
        arrow(),
        size("arrow_size", 2.5),
        size("extension_offset", 0.625),
        size("extension_beyond", 1.25),
        FieldDecl::new(
            "unit_format",
            variants(&[
                "decimal",
                "engineering",
                "architectural",
                "fractional",
                "scientific",
            ]),
        )
        .with_default(Value::Enum(0)),
        FieldDecl::new("precision", ValueType::I64)
            .with_default(Value::I64(2))
            .with_bound(Bound::Int { min: 0, max: 8 }),
        real("measurement_scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
        real("overall_scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
        text("description", TEXT_BYTES),
        size("rounding", 0.0),
        flag("suppress_leading_zeros", false),
        flag("suppress_trailing_zeros", false),
        FieldDecl::new("decimal_separator", ValueType::String)
            .with_default(Value::String(".".into()))
            .with_bound(Bound::Length { max: 4 }),
        flag("unit_suffix", false),
        FieldDecl::new("angular_precision", ValueType::I64)
            .with_default(Value::I64(0))
            .with_bound(Bound::Int { min: 0, max: 8 }),
        FieldDecl::new("text_vertical", variants(&["centered", "above"]))
            .with_default(Value::Enum(0)),
        FieldDecl::new(
            "tolerance",
            variants(&["none", "symmetric", "deviation", "limits"]),
        )
        .with_default(Value::Enum(0)),
        size("tolerance_upper", 0.0),
        size("tolerance_lower", 0.0),
    ]
}

fn leader_style_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        style_field("text_style", TEXT_STYLE),
        size("text_height", 2.5),
        arrow(),
        size("arrow_size", 2.5),
        flag("landing", true),
        size("landing_length", 2.5),
        FieldDecl::new("content", variants(&["text", "block", "tolerance", "none"]))
            .with_default(Value::Enum(0)),
        text("description", TEXT_BYTES),
        size("text_gap", 0.625),
    ]
}

fn row(name: &str) -> FieldDecl {
    let record = ValueType::Record(vec![
        style_field("text_style", TEXT_STYLE),
        size("text_height", 2.5),
        FieldDecl::new("alignment", variants(&["left", "center", "right"]))
            .with_default(Value::Enum(1)),
        flag("borders", true),
        text("format", NAME_BYTES),
    ]);
    FieldDecl::new(name, record).optional()
}

fn table_style_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        size("cell_margin", 1.0),
        row("title"),
        row("header"),
        row("data"),
        text("description", TEXT_BYTES),
    ]
}

/// The style declarations.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (TEXT_STYLE, text_style_fields(), vec![]),
        (DIM_STYLE, dim_style_fields(), vec![]),
        (LEADER_STYLE, leader_style_fields(), vec![]),
        (TABLE_STYLE, table_style_fields(), vec![]),
    ]
}

/// The field names of the style type `key`, in declared order.
#[must_use]
pub fn field_names(key: &str) -> Vec<String> {
    let fields = match key {
        TEXT_STYLE => text_style_fields(),
        DIM_STYLE => dim_style_fields(),
        LEADER_STYLE => leader_style_fields(),
        TABLE_STYLE => table_style_fields(),
        _ => Vec::new(),
    };
    fields.into_iter().map(|f| f.name).collect()
}

/// The effective data of a style: its type, its own field values in declared order, and the
/// effective data of the styles it references, in the order the references appear.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The style.
    pub id: ObjectId,
    /// Its type key.
    pub type_key: String,
    /// Its field values.
    pub values: Vec<Option<Value>>,
    /// The styles it references with their effective data.
    pub referenced: Vec<(ObjectId, Arc<Effective>)>,
}

impl Derived for Effective {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text (floats print round-trip); a `Hash` impl over
        // field values would avoid the allocation when profiles show it matters.
        content_hash(&format!("{self:?}"))
    }
}

fn references(value: &Value, out: &mut Vec<ObjectId>) {
    match value {
        Value::Reference(id) => out.push(*id),
        Value::List(items) => items.iter().for_each(|v| references(v, out)),
        Value::Record(fields) => fields.iter().flatten().for_each(|v| references(v, out)),
        Value::Map(map) => map.values().for_each(|v| references(v, out)),
        _ => {}
    }
}

/// The node of the effective data of the style `id`.
///
/// # Errors
///
/// `doc.graph.malformed_key` never happens for the constant key; it is passed on.
pub fn effective_node(id: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(id, DerivedKey::new(EFFECTIVE)?))
}

fn evaluate(ctx: &EvalContext<'_>) -> Result<DerivedValue> {
    let id = ctx
        .object_id()
        .ok_or_else(|| arc_base::Error::new(crate::error::codes::OBJECT_WRONG_TYPE))?;
    let type_key = ctx
        .type_key(id)
        .map(|k| k.as_str().to_owned())
        .unwrap_or_default();
    let mut values = Vec::new();
    for name in field_names(&type_key) {
        values.push(ctx.field(id, &name)?.cloned());
    }
    let mut targets = Vec::new();
    values
        .iter()
        .flatten()
        .for_each(|v| references(v, &mut targets));
    let mut referenced = Vec::new();
    for target in targets {
        let node = ctx.node(&effective_node(target)?)?;
        if let Some(data) = node.downcast_ref::<Effective>() {
            referenced.push((target, Arc::new(data.clone())));
        }
    }
    Ok(Arc::new(Effective {
        id,
        type_key,
        values,
        referenced,
    }))
}

/// Registers the [`EFFECTIVE`] evaluator for every style type.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when one is registered already.
pub fn register_evaluators(evaluators: &mut Evaluators) -> Result<()> {
    for key in STYLES {
        evaluators.register_object(TypeKey::new(*key)?, DerivedKey::new(EFFECTIVE)?, evaluate)?;
    }
    Ok(())
}
