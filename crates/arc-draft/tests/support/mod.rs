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

/// Saves the document of `f` to a package, opens it again and asserts that every object
/// (identifier, type key, header, values) is unchanged.
pub fn round_trip(f: &Fixture) {
    use arc_base::Limits;
    use arc_doc::package::{LockOwner, Metadata, Mode, Package};
    use arc_doc::{LoadOptions, Snapshot};

    fn dump(snapshot: &Snapshot) -> Vec<String> {
        let mut rows: Vec<String> = snapshot
            .iter()
            .map(|(handle, object)| {
                format!(
                    "{} {} {:?} {:?}",
                    object.id,
                    snapshot.type_key(handle).unwrap().as_str(),
                    object.header,
                    object.values
                )
            })
            .collect();
        rows.sort();
        rows
    }
    let before = f.document.snapshot();
    let dir = std::env::temp_dir().join(format!(
        "arc-draft-text-{}-{}",
        std::process::id(),
        before.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("drawing.arc");
    let owner = LockOwner {
        user: "u".into(),
        host: "h".into(),
        application: "arc-draft tests".into(),
        time_unix_ms: 0,
    };
    let mut package = Package::create(&path, &Metadata::default(), &owner).unwrap();
    package
        .save_document(&f.document, &[], None, &mut |_| Ok(()))
        .unwrap();
    drop(package);
    let package = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let reopened = package
        .open_document(
            types(),
            Arc::new(Evaluators::new()),
            &LoadOptions::default(),
            DocumentConfig::default(),
        )
        .unwrap();
    assert!(reopened.load_diagnostics().is_empty());
    assert_eq!(dump(&reopened.snapshot()), dump(&before));
    drop(package);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Makes the entity use its own color, lineweight and linetype.
pub fn pen(
    f: &mut Fixture,
    id: ObjectId,
    color: [f64; 4],
    lineweight: f64,
    linetype: Option<ObjectId>,
) {
    f.set(
        "core.draft.entity.modify",
        &[id],
        "color_by",
        json!("object"),
    );
    f.set("core.draft.entity.modify", &[id], "color", json!(color));
    f.set(
        "core.draft.entity.modify",
        &[id],
        "lineweight_by",
        json!("object"),
    );
    f.set(
        "core.draft.entity.modify",
        &[id],
        "lineweight",
        json!(lineweight),
    );
    if let Some(l) = linetype {
        f.set(
            "core.draft.entity.modify",
            &[id],
            "linetype_by",
            json!("object"),
        );
        f.set(
            "core.draft.entity.modify",
            &[id],
            "linetype",
            json!(l.to_string()),
        );
    }
}

pub fn segment_json(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap(),
    ))
}

pub const BLACK: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

/// A model space in meters, a landscape A4 layout in millimeters (sheet 297 x 210) and one
/// viewport at 1:100 showing the model.
///
/// The viewport is 200 x 100 mm about (150, 100) and shows the model point (5, 0) at its
/// center, so a model meter is 10 sheet millimeters: `model_line` from (0, 0) to (10, 0) meters
/// is the sheet segment (100, 100)-(200, 100), dashed with a pattern of 0.5 m dash and 0.25 m
/// gap (5 mm and 2.5 mm on the sheet); `far_line` from (0, 2) to (30, 2) leaves the viewport at
/// x = 250; `sheet_line` is a solid 1 mm line from (10, 10) to (60, 10) on the sheet.
pub struct Scene {
    pub f: Fixture,
    pub model: ObjectId,
    pub sheet: ObjectId,
    pub settings: ObjectId,
    pub viewport: ObjectId,
    pub dashed: ObjectId,
    pub model_line: ObjectId,
    pub far_line: ObjectId,
    pub sheet_line: ObjectId,
}

pub fn plot_scene(mut f: Fixture) -> Scene {
    let model = f.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "meter"}),
    );
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [
            {"kind": "dash", "length": 0.5}, {"kind": "gap", "length": 0.25}]}),
    );
    let created = f
        .run(
            "core.draft.layout.create",
            json!({"name": "sheet", "paper_width": 210.0, "paper_height": 297.0, "orientation": "landscape"}),
        )
        .unwrap();
    let sheet: ObjectId = created["space"].as_str().unwrap().parse().unwrap();
    let settings: ObjectId = created["layout"].as_str().unwrap().parse().unwrap();
    let viewport = f.create(
        "core.draft.viewport.create",
        json!({"layout": sheet.to_string(), "model": model.to_string(), "center": [150.0, 100.0],
               "width": 200.0, "height": 100.0, "view_center": [5.0, 0.0], "scale": 0.01}),
    );
    let model_line = f.entity(model, segment_json(0.0, 0.0, 10.0, 0.0));
    pen(&mut f, model_line, BLACK, 0.5, Some(dashed));
    let far_line = f.entity(model, segment_json(0.0, 2.0, 30.0, 2.0));
    pen(&mut f, far_line, BLACK, 0.5, None);
    let sheet_line = f.entity(sheet, segment_json(10.0, 10.0, 60.0, 10.0));
    pen(&mut f, sheet_line, BLACK, 1.0, None);
    Scene {
        f,
        model,
        sheet,
        settings,
        viewport,
        dashed,
        model_line,
        far_line,
        sheet_line,
    }
}
