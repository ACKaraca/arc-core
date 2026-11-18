//! A document with the drafting types and commands, the DXF commands, a session with file
//! roots in a temporary directory, and helpers to build neutral drawings.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_base::{Cancel, Limits, ObjectId};
use arc_command::{Invocation, Invoker, Outcome, Permission, Registry, Roots, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_io_dxf::model::{Block, BlockBegin, Circle, Layer, Line, SeqEnd, Table, TableEntry};
use arc_io_dxf::{Drawing, Entity, EntityKind, Group, WriteOptions};
use serde_json::{Value, json};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn types() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    arc_draft::register_types(&mut registry).unwrap();
    Arc::new(registry)
}

/// A temporary directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("arc-draft-dxf-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Env {
    pub document: Document,
    pub commands: Registry,
    pub session: Session,
    pub dir: TempDir,
    seed: u64,
}

impl Env {
    pub fn new() -> Self {
        let config = DocumentConfig {
            workers: 0,
            ..DocumentConfig::default()
        };
        let document = Document::open(
            Snapshot::empty(types()),
            Arc::new(Evaluators::new()),
            config,
        )
        .unwrap();
        for validator in arc_draft::validate::validators(arc_draft::validate::DEFAULT_MAX_NESTING) {
            document.add_validator(validator);
        }
        let mut commands = Registry::new();
        arc_draft::register_commands(&mut commands).unwrap();
        arc_draft_dxf::register_commands(&mut commands).unwrap();
        let dir = TempDir::new();
        let mut session = Session::new(
            Surface::Automation,
            [
                Permission::Edit,
                Permission::Read,
                Permission::FileRead,
                Permission::FileWrite,
            ],
        );
        session.roots = Roots::new([&dir.0], [&dir.0]).unwrap();
        Self {
            document,
            commands,
            session,
            dir,
            seed: 1,
        }
    }

    pub fn invoke(&mut self, command: &str, params: Value) -> arc_base::Result<Outcome> {
        self.seed += 1;
        let invocation = Invocation {
            seed: self.seed,
            ..Invocation::new(command, params)
        };
        Invoker::new(&self.commands).invoke(&self.document, &self.session, invocation)
    }

    pub fn invoke_with(&mut self, invocation: Invocation) -> arc_base::Result<Outcome> {
        Invoker::new(&self.commands).invoke(&self.document, &self.session, invocation)
    }

    pub fn run(&mut self, command: &str, params: Value) -> Value {
        self.invoke(command, params).unwrap().value
    }

    pub fn create(&mut self, command: &str, params: Value) -> ObjectId {
        self.run(command, params)["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    pub fn space(&mut self, name: &str, unit: &str) -> ObjectId {
        self.create(
            "core.draft.space.create",
            json!({"name": name, "kind": "model", "unit": unit}),
        )
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.0.join(name)
    }

    pub fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    }

    pub fn import(&mut self, path: &str, space: ObjectId) -> arc_base::Result<Outcome> {
        self.invoke(
            "core.draft.import.dxf",
            json!({"path": path, "space": space.to_string()}),
        )
    }

    pub fn snapshot(&self) -> Snapshot {
        self.document.snapshot()
    }

    /// The model space exported again as a neutral drawing, to read what an import created.
    pub fn exported(&self, space: ObjectId) -> Drawing {
        arc_draft_dxf::export_drawing(&self.snapshot(), space)
            .unwrap()
            .drawing
    }
}

pub fn cancelled() -> Cancel {
    let c = Cancel::new();
    c.cancel();
    c
}

pub fn limits_with_records(n: u64) -> Limits {
    Limits {
        record_count: n,
        ..Limits::DEFAULT
    }
}

pub fn line(start: [f64; 3], end: [f64; 3]) -> Entity {
    Entity::new(EntityKind::Line(Line { start, end }))
}

pub fn circle(center: [f64; 3], radius: f64) -> Entity {
    Entity::new(EntityKind::Circle(Circle { center, radius }))
}

pub fn on_layer(mut e: Entity, layer: &str) -> Entity {
    e.props.layer = layer.to_owned();
    e
}

pub fn with_handle(mut e: Entity, handle: u64) -> Entity {
    e.head.handle = Some(arc_io_dxf::Handle(handle));
    e
}

pub fn block(name: &str, entities: Vec<Entity>) -> Block {
    Block {
        begin: Entity::new(EntityKind::Block(BlockBegin {
            name: name.to_owned(),
            ..BlockBegin::default()
        })),
        entities,
        end: Entity::new(EntityKind::BlockEnd(SeqEnd::default())),
    }
}

pub fn layer_entry(name: &str, aci: i16) -> TableEntry<Layer> {
    TableEntry {
        name: name.to_owned(),
        data: Layer {
            color: aci,
            linetype: "Continuous".to_owned(),
            ..Layer::default()
        },
        ..TableEntry::default()
    }
}

/// A drawing with the layer table `layers` and `entities` in the model space, with the
/// insertion units `units` when given.
pub fn drawing(entities: Vec<Entity>, units: Option<i64>) -> Drawing {
    let mut d = Drawing {
        version: arc_io_dxf::Version::R2018,
        ..Drawing::default()
    };
    if let Some(u) = units {
        d.header.push(arc_io_dxf::model::HeaderVariable {
            name: "$INSUNITS".to_owned(),
            values: vec![Group::int(70, u)],
        });
    }
    d.tables.layers = Some(Table {
        entries: vec![layer_entry("0", 7), layer_entry("WALLS", 1)],
        ..Table::default()
    });
    d.entities = entities;
    d
}

pub fn bytes(d: &Drawing) -> Vec<u8> {
    arc_io_dxf::write(d, &mut WriteOptions::default())
        .unwrap()
        .bytes
}
