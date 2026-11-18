//! Sample commands over a test registry of `arc-doc` types, shared by the integration tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test fixtures; each test file uses a part"
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arc_base::{Dimension, Error, ObjectId, Quantity, Unit};
use arc_command::{
    Clock, CommandSpec, Context, Disabled, IdParam, Invocation, Invoker, Outcome, Permission,
    ProgressSink, QuantityParam, Registry, Session, Surface, UndoBehavior,
};
use arc_doc::graph::Evaluators;
use arc_doc::{
    Bound, Document, FieldDecl, FieldPath, Header, Schema, SchemaVersion, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

pub const ITEM: &str = "test.item";

pub fn types() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("length", ValueType::Quantity(Dimension::LENGTH))
                .with_bound(Bound::Real { min: 0.0, max: 1e6 }),
            FieldDecl::new("name", ValueType::String).with_bound(Bound::Length { max: 64 }),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new(ITEM).unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

pub fn document() -> Document {
    Document::new(types(), Arc::new(Evaluators::new())).unwrap()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Create {
    /// The item name.
    pub name: String,
    pub length: QuantityParam,
}

#[derive(Serialize, JsonSchema)]
pub struct Created {
    pub id: IdParam,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLength {
    pub id: IdParam,
    pub length: QuantityParam,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: IdParam,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Nothing {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Steps {
    #[schemars(range(max = 1000))]
    pub steps: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct Summary {
    pub count: u64,
    pub total_length: f64,
}

/// A value whose decoding panics.
#[derive(JsonSchema)]
pub struct Boom;

impl<'de> Deserialize<'de> for Boom {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        panic!("decode fault")
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Explosive {
    pub boom: Boom,
}

/// A result whose encoding panics.
#[derive(JsonSchema)]
pub struct ExplosiveResult;

impl Serialize for ExplosiveResult {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        panic!("encode fault")
    }
}

fn length_of(ctx: &Context<'_>, param: &QuantityParam) -> arc_base::Result<Quantity> {
    ctx.quantity(param, Dimension::LENGTH)
}

fn create(ctx: &mut Context<'_>, p: Create) -> arc_base::Result<Created> {
    let length = length_of(ctx, &p.length)?;
    let id: ObjectId = ctx.new_id();
    ctx.txn()?.create(
        ITEM,
        id,
        Header::default(),
        vec![Some(Value::Quantity(length)), Some(Value::String(p.name))],
    )?;
    ctx.set_selection_after(vec![id]);
    Ok(Created { id: IdParam(id) })
}

fn summary(ctx: &Context<'_>) -> arc_base::Result<Summary> {
    let mut out = Summary {
        count: 0,
        total_length: 0.0,
    };
    for (_, object) in ctx.snapshot().iter_type(ITEM)? {
        out.count += 1;
        if let Some(Some(Value::Quantity(q))) = object.values.first() {
            out.total_length += q.value();
        }
    }
    Ok(out)
}

/// The sample commands. The `ITEM` commands are deterministic; the others exercise failures.
pub fn registry() -> Registry {
    let mut r = Registry::new();
    let specs = vec![
        CommandSpec::new(
            "test.item.create",
            "test-create",
            "test-create-description",
            create,
        )
        .with_example(json!({"name": "a", "length": 2.4}))
        .with_example(json!({"name": "b", "length": "2400 mm"})),
        CommandSpec::new(
            "test.item.set_length",
            "test-set-length",
            "test-set-length-description",
            |ctx, p: SetLength| {
                let length = length_of(ctx, &p.length)?;
                ctx.txn()?.modify(
                    p.id.0,
                    FieldPath::new("length"),
                    Some(Value::Quantity(length)),
                )
            },
        )
        .coalesced("test.length")
        .with_enabled(|probe| {
            if probe.snapshot.is_empty() {
                Err(Disabled::new("test-nothing-to-change").with_arg("count", 0_i64))
            } else {
                Ok(())
            }
        })
        .with_example(json!({"id": "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c", "length": "3'6\""})),
        CommandSpec::new(
            "test.item.delete",
            "test-delete",
            "test-delete-description",
            |ctx, p: Target| ctx.txn()?.delete(p.id.0),
        ),
        CommandSpec::new(
            "test.item.summary",
            "test-summary",
            "test-summary-description",
            |ctx, _: Nothing| summary(ctx),
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
        .transparent()
        .with_example(json!({})),
        CommandSpec::new(
            "test.item.random",
            "test-random",
            "test-random-description",
            |ctx, _: Nothing| Ok(ctx.random_u64()),
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
        .with_exposure([Surface::Automation]),
        CommandSpec::new(
            "test.fail.always",
            "test-fail",
            "test-fail-description",
            |ctx, _: Nothing| -> arc_base::Result<()> {
                create(
                    ctx,
                    Create {
                        name: "doomed".into(),
                        length: QuantityParam::Si(1.0),
                    },
                )?;
                Err(Error::new(arc_doc::error::codes::OBJECT_NOT_FOUND))
            },
        ),
        CommandSpec::new(
            "test.fault.panic",
            "test-panic",
            "test-panic-description",
            |ctx, _: Nothing| -> arc_base::Result<()> {
                create(
                    ctx,
                    Create {
                        name: "doomed".into(),
                        length: QuantityParam::Si(1.0),
                    },
                )?;
                panic!("guard test fault")
            },
        ),
        CommandSpec::new(
            "test.fault.decode",
            "test-panic",
            "test-panic-description",
            |_, _: Explosive| Ok(()),
        )
        .with_exposure([Surface::Automation]),
        CommandSpec::new(
            "test.fault.encode",
            "test-panic",
            "test-panic-description",
            |_, _: Nothing| Ok(ExplosiveResult),
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
        .with_exposure([Surface::Automation]),
        CommandSpec::new(
            "test.fault.predicate",
            "test-panic",
            "test-panic-description",
            |_, _: Nothing| Ok(()),
        )
        .with_enabled(|_| panic!("predicate fault"))
        .with_exposure([Surface::Automation]),
        CommandSpec::new(
            "test.long.run",
            "test-long",
            "test-long-description",
            |ctx, p: Steps| {
                for step in 0..p.steps {
                    ctx.checkpoint()?;
                    create(
                        ctx,
                        Create {
                            name: format!("s{step}"),
                            length: QuantityParam::Si(f64::from(step)),
                        },
                    )?;
                    ctx.progress(f64::from(step + 1) / f64::from(p.steps), "test-long-step");
                }
                Ok(p.steps)
            },
        )
        .with_example(json!({"steps": 3})),
        CommandSpec::new(
            "test.file.touch",
            "test-touch",
            "test-touch-description",
            |_, _: Nothing| Ok(()),
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read, Permission::FileWrite]),
        CommandSpec::new(
            "test.hidden.noop",
            "test-hidden",
            "test-hidden-description",
            |_, _: Nothing| Ok(()),
        )
        .with_exposure([]),
    ];
    for spec in specs {
        r.register(spec).unwrap();
    }
    r.add_deprecated_alias("test.item.make", "test.item.create")
        .unwrap();
    r
}

/// Every permission class.
pub fn all_permissions() -> Vec<Permission> {
    vec![
        Permission::Read,
        Permission::Edit,
        Permission::FileRead,
        Permission::FileWrite,
        Permission::Network,
        Permission::Process,
        Permission::Settings,
    ]
}

/// An automation session with every class and millimeters as the context length unit.
pub fn session() -> Session {
    let mut session = Session::new(Surface::Automation, all_permissions());
    session.units = vec![*Unit::by_symbol("mm").unwrap()];
    session
}

pub fn invoke(
    registry: &Registry,
    doc: &Document,
    session: &Session,
    command: &str,
    params: serde_json::Value,
) -> arc_base::Result<Outcome> {
    Invoker::new(registry).invoke(doc, session, Invocation::new(command, params))
}

/// A clock that advances `step` milliseconds each time it is read.
pub struct SteppingClock {
    pub now: AtomicU64,
    pub step: u64,
}

impl Clock for SteppingClock {
    fn now_unix_millis(&self) -> u64 {
        self.now.fetch_add(self.step, Ordering::SeqCst)
    }
}

/// Collects progress reports.
#[derive(Default)]
pub struct Reports(pub Mutex<Vec<(f64, String)>>);

impl ProgressSink for Reports {
    fn progress(&self, fraction: f64, message_id: &str) {
        self.0
            .lock()
            .unwrap()
            .push((fraction, message_id.to_owned()));
    }
}
