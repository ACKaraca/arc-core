//! A document with the drafting types, validators and commands, and helpers to run commands.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::math::{Point2, Vector2};
use arc_command::{Invocation, Invoker, Permission, Registry, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_draft::display::DisplayOptions;
use arc_geom::{
    Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, Line2, Polyline2, PolylineVertex, Ray2,
    Segment2,
};
use serde_json::{Value, json};

pub fn types() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    arc_draft::register_types(&mut registry).unwrap();
    Arc::new(registry)
}

pub struct Fixture {
    pub document: Document,
    pub commands: Registry,
    pub session: Session,
    seed: u64,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_nesting(arc_draft::validate::DEFAULT_MAX_NESTING)
    }

    pub fn with_nesting(limit: u32) -> Self {
        Self::build(limit, Evaluators::new())
    }

    /// A document with the drafting evaluators and no regeneration workers: nodes evaluate
    /// only when demanded.
    pub fn drafting() -> Self {
        let mut evaluators = Evaluators::new();
        arc_draft::register_evaluators(&mut evaluators, DisplayOptions::default()).unwrap();
        Self::with_evaluators(evaluators)
    }

    pub fn with_evaluators(evaluators: Evaluators) -> Self {
        Self::build(arc_draft::validate::DEFAULT_MAX_NESTING, evaluators)
    }

    fn build(limit: u32, evaluators: Evaluators) -> Self {
        let config = DocumentConfig {
            workers: 0,
            ..DocumentConfig::default()
        };
        let document =
            Document::open(Snapshot::empty(types()), Arc::new(evaluators), config).unwrap();
        for validator in arc_draft::validate::validators(limit) {
            document.add_validator(validator);
        }
        let mut commands = Registry::new();
        arc_draft::register_commands(&mut commands).unwrap();
        Self {
            document,
            commands,
            session: Session::new(Surface::Automation, [Permission::Edit, Permission::Read]),
            seed: 1,
        }
    }

    pub fn run(&mut self, command: &str, params: Value) -> arc_base::Result<Value> {
        self.seed += 1;
        let invocation = Invocation {
            seed: self.seed,
            ..Invocation::new(command, params)
        };
        Invoker::new(&self.commands)
            .invoke(&self.document, &self.session, invocation)
            .map(|o| o.value)
    }

    pub fn create(&mut self, command: &str, params: Value) -> ObjectId {
        let value = self.run(command, params).unwrap();
        value["id"].as_str().unwrap().parse().unwrap()
    }

    pub fn space(&mut self, name: &str) -> ObjectId {
        self.create(
            "core.draft.space.create",
            json!({"name": name, "kind": "model", "unit": "millimeter"}),
        )
    }

    pub fn entity(&mut self, owner: ObjectId, geometry: Value) -> ObjectId {
        self.create(
            "core.draft.entity.create",
            json!({"owner": owner.to_string(), "geometry": geometry}),
        )
    }

    pub fn line(&mut self, owner: ObjectId) -> ObjectId {
        self.entity(owner, curve_json(&every_curve()[0]))
    }

    /// Sets `field` of `objects` with the modify command `command`.
    pub fn set(&mut self, command: &str, objects: &[ObjectId], field: &str, value: Value) {
        let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
        self.run(
            command,
            json!({"objects": ids, "field": field, "value": value}),
        )
        .unwrap();
    }

    pub fn values(&self, id: ObjectId) -> Vec<Option<arc_doc::Value>> {
        let snapshot = self.document.snapshot();
        snapshot
            .get(snapshot.resolve(id).unwrap())
            .unwrap()
            .values
            .clone()
    }
}

pub fn curve_json(curve: &Curve2) -> Value {
    serde_json::to_value(curve).unwrap()
}

/// One curve of every kind.
pub fn every_curve() -> Vec<Curve2> {
    let o = Point2::new(0.0, 0.0);
    let x = Vector2::new(1.0, 0.0);
    let ellipse = Ellipse2::new(o, Vector2::new(2.0, 0.0), 0.5).unwrap();
    let vertices = vec![
        PolylineVertex::new(o, 0.5),
        PolylineVertex {
            point: Point2::new(3.0, 0.0),
            bulge: 0.0,
            start_width: 0.1,
            end_width: 0.2,
        },
        PolylineVertex::new(Point2::new(3.0, 2.0), 0.0),
    ];
    vec![
        Curve2::Segment(Segment2::new(o, Point2::new(4.0, 0.0)).unwrap()),
        Curve2::Ray(Ray2::new(o, x).unwrap()),
        Curve2::Line(Line2::new(o, x).unwrap()),
        Curve2::Circle(Circle2::new(o, 2.0).unwrap()),
        Curve2::Arc(Arc2::new(o, 2.0, 0.5, 2.0).unwrap()),
        Curve2::Ellipse(ellipse),
        Curve2::EllipticalArc(EllipticalArc2::new(ellipse, 0.2, 1.5).unwrap()),
        Curve2::Polyline(Polyline2::new(vertices, false).unwrap()),
    ]
}
