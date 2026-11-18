//! The sample document type set of the reference shell, shared by the windowed shell and the
//! headless CLI (`apps/arc-shell-cli`).
//!
//! The sample types are a point, a line and a layer that points and lines refer to; the sample
//! commands create, move and delete them. Types are registered through the public `arc-doc`
//! type registry and commands through the public `arc-command` registry, and every change goes
//! through a command in a transaction. [`product`] bundles both for
//! [`arc_command::headless::run`].

use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::math::Point3;
use arc_command::headless::Product;
use arc_command::{Clock, CommandSpec, Context, IdParam, Permission, Registry, UndoBehavior};
use arc_doc::graph::Evaluators;
use arc_doc::package::LockOwner;
use arc_doc::{
    Bound, Cardinality, FieldDecl, FieldPath, Header, OnCopy, OnDelete, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Strength, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The type key of a layer: field `name`.
pub const LAYER: &str = "shell.layer";
/// The type key of a point: fields `position` and the optional `layer`.
pub const POINT: &str = "shell.point";
/// The type key of a line: fields `start`, `end` and the optional `layer`.
pub const LINE: &str = "shell.line";

/// The most bytes of a layer name.
const NAME_BYTES: u64 = 256;

/// The optional reference of a point or line to its layer; deleting the layer clears it.
fn layer_field() -> FieldDecl {
    let spec = ReferenceSpec {
        targets: TypeKey::new(LAYER).into_iter().collect(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    };
    FieldDecl::new("layer", ValueType::Reference(spec)).optional()
}

/// The sample types, version 1.0 each.
///
/// # Errors
///
/// The registry's errors; none occur for these declarations.
pub fn types() -> arc_base::Result<TypeRegistry> {
    let v1 = SchemaVersion::new(1, 0);
    let declarations = [
        (
            LAYER,
            vec![
                FieldDecl::new("name", ValueType::String)
                    .with_bound(Bound::Length { max: NAME_BYTES }),
            ],
        ),
        (
            POINT,
            vec![FieldDecl::new("position", ValueType::Point3), layer_field()],
        ),
        (
            LINE,
            vec![
                FieldDecl::new("start", ValueType::Point3),
                FieldDecl::new("end", ValueType::Point3),
                layer_field(),
            ],
        ),
    ];
    let mut registry = TypeRegistry::new();
    for (key, fields) in declarations {
        registry.register(TypeDecl::new(TypeKey::new(key)?, Schema::new(v1, fields)))?;
    }
    Ok(registry)
}

/// Parameters of `shell.layer.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLayer {
    /// The layer name.
    #[schemars(length(max = 256))]
    pub name: String,
}

/// Parameters of `shell.point.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePoint {
    /// The position in meters.
    pub position: [f64; 3],
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `shell.line.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLine {
    /// The start point in meters.
    pub start: [f64; 3],
    /// The end point in meters.
    pub end: [f64; 3],
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `shell.object.move`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Move {
    /// A point or line.
    pub id: IdParam,
    /// The displacement in meters.
    pub offset: [f64; 3],
}

/// Parameters of `shell.object.delete`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    /// The object to delete.
    pub id: IdParam,
}

/// The result of a creating command.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Created {
    /// The new object.
    pub id: IdParam,
}

fn point(p: [f64; 3]) -> Value {
    let [x, y, z] = p;
    Value::Point3(Point3::new(x, y, z))
}

fn layer(id: Option<IdParam>) -> Option<Value> {
    id.map(|IdParam(id)| Value::Reference(id))
}

/// Creates an object of `key` with `values` and selects it.
fn create(
    ctx: &mut Context<'_>,
    key: &str,
    values: Vec<Option<Value>>,
) -> arc_base::Result<Created> {
    let id: ObjectId = ctx.new_id();
    ctx.txn()?.create(key, id, Header::default(), values)?;
    ctx.set_selection_after(vec![id]);
    Ok(Created { id: IdParam(id) })
}

/// Adds `offset` to the point fields of the point or line `id`. The sum is checked by the
/// store: a non-finite coordinate is refused.
fn move_object(ctx: &mut Context<'_>, p: Move) -> arc_base::Result<()> {
    let IdParam(id) = p.id;
    let txn = ctx.txn()?;
    let snapshot = txn.snapshot();
    let handle = snapshot.resolve(id)?;
    let object = Arc::clone(snapshot.get(handle)?);
    let fields: &[&str] = match snapshot.type_key(handle).map(TypeKey::as_str) {
        Some(POINT) => &["position"],
        Some(LINE) => &["start", "end"],
        _ => &[],
    };
    for (index, name) in fields.iter().enumerate() {
        if let Some(Some(Value::Point3(at))) = object.values.get(index) {
            let [dx, dy, dz] = p.offset;
            let moved = Value::Point3(Point3::new(at.x + dx, at.y + dy, at.z + dz));
            txn.modify(id, FieldPath::new(*name), Some(moved))?;
        }
    }
    Ok(())
}

fn delete(ctx: &mut Context<'_>, p: Delete) -> arc_base::Result<()> {
    let limits = *ctx.limits();
    let txn = ctx.txn()?;
    txn.delete(p.id.0)?;
    txn.apply_delete_rules(&limits)?;
    Ok(())
}

/// The sample commands: `shell.layer.create`, `shell.point.create`, `shell.line.create`,
/// `shell.object.move` and `shell.object.delete`, all undoable with the permission class
/// `edit`.
///
/// # Errors
///
/// The registry's errors; none occur for these specifications.
pub fn registry() -> arc_base::Result<Registry> {
    let specs = [
        CommandSpec::new(
            "shell.layer.create",
            "shell-layer-create",
            "shell-layer-create-description",
            |ctx, p: CreateLayer| create(ctx, LAYER, vec![Some(Value::String(p.name))]),
        )
        .with_example(serde_json::json!({"name": "walls"})),
        CommandSpec::new(
            "shell.point.create",
            "shell-point-create",
            "shell-point-create-description",
            |ctx, p: CreatePoint| create(ctx, POINT, vec![Some(point(p.position)), layer(p.layer)]),
        )
        .with_example(serde_json::json!({"position": [0.0, 0.0, 0.0]})),
        CommandSpec::new(
            "shell.line.create",
            "shell-line-create",
            "shell-line-create-description",
            |ctx, p: CreateLine| {
                let values = vec![Some(point(p.start)), Some(point(p.end)), layer(p.layer)];
                create(ctx, LINE, values)
            },
        )
        .with_example(serde_json::json!({"start": [0.0, 0.0, 0.0], "end": [1.0, 0.0, 0.0]})),
        CommandSpec::new(
            "shell.object.move",
            "shell-object-move",
            "shell-object-move-description",
            move_object,
        ),
        CommandSpec::new(
            "shell.object.delete",
            "shell-object-delete",
            "shell-object-delete-description",
            delete,
        ),
    ];
    let mut registry = Registry::new();
    for spec in specs {
        registry.register(
            spec.with_undo(UndoBehavior::Undoable)
                .with_permissions([Permission::Edit]),
        )?;
    }
    Ok(registry)
}

/// The product the binary runs: the sample types and commands with `clock` and the edit-lock
/// `owner`.
///
/// # Errors
///
/// The registries' errors; none occur for the sample set.
pub fn product(clock: Arc<dyn Clock>, owner: LockOwner) -> arc_base::Result<Product> {
    Ok(Product {
        name: "arc-shell".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        types: Arc::new(types()?),
        evaluators: Arc::new(Evaluators::new()),
        registry: registry()?,
        clock,
        owner,
    })
}
