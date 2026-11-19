//! The headless render of a drafting space (CORE-APP-026): an arc of a three-quarter turn is
//! framed by its exact bounds, so its bulge is inside the image and nothing touches the border.
//! Skips when the machine has no graphics adapter.

#![cfg(feature = "render")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::sync::Arc;

use arc_app::render::render_drawing;
use arc_base::math::Point2;
use arc_command::{Invocation, Invoker, Permission, Registry, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_geom::{Arc2, Curve2};
use arc_gpu::draft::{DraftOptions, space_extractions};
use serde_json::{Value, json};

/// Runs a drafting command on a fresh document and returns its result value.
fn run(document: &Document, commands: &Registry, command: &str, params: Value) -> Value {
    let session = Session::new(Surface::Automation, [Permission::Edit, Permission::Read]);
    Invoker::new(commands)
        .invoke(document, &session, Invocation::new(command, params))
        .unwrap()
        .value
}

/// A model space with one dark arc: a three-quarter turn of radius 5 about the origin, from the
/// positive x axis counterclockwise to the negative y axis, so its ends are (5, 0) and (0, −5).
fn drawing() -> (Snapshot, arc_base::ObjectId) {
    let mut types = TypeRegistry::new();
    arc_draft::register_types(&mut types).unwrap();
    let config = DocumentConfig {
        workers: 0,
        ..DocumentConfig::default()
    };
    let document = Document::open(
        Snapshot::empty(Arc::new(types)),
        Arc::new(Evaluators::new()),
        config,
    )
    .unwrap();
    let mut commands = Registry::new();
    arc_draft::register_commands(&mut commands).unwrap();
    let space = run(
        &document,
        &commands,
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "millimeter"}),
    );
    let space: arc_base::ObjectId = space["id"].as_str().unwrap().parse().unwrap();
    let arc =
        Curve2::Arc(Arc2::new(Point2::origin(), 5.0, 0.0, 1.5 * std::f64::consts::PI).unwrap());
    let entity = run(
        &document,
        &commands,
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": serde_json::to_value(&arc).unwrap()}),
    );
    let entity: arc_base::ObjectId = entity["id"].as_str().unwrap().parse().unwrap();
    run(
        &document,
        &commands,
        "core.draft.entity.modify",
        json!({"objects": [entity.to_string()], "field": "color_by", "value": "object"}),
    );
    run(
        &document,
        &commands,
        "core.draft.entity.modify",
        json!({"objects": [entity.to_string()], "field": "color", "value": [0.0, 0.0, 0.0, 1.0]}),
    );
    let snapshot = document.snapshot();
    (snapshot, space)
}

#[test]
fn an_arc_of_a_drafting_space_is_framed_by_its_bulge_and_drawn_inside_the_image() {
    let (snapshot, space) = drawing();
    let drawing = move |snapshot: &Snapshot| {
        let objects = space_extractions(snapshot, space, &DraftOptions::default())?;
        Ok(objects
            .into_iter()
            .map(|(id, d)| (id, d.extraction))
            .collect())
    };
    let rendered = match render_drawing(&snapshot, &drawing, 200, 200) {
        Ok(rendered) => rendered,
        // No usable adapter: nothing can be rendered here, so the test has nothing to check.
        Err(error) if error.code().starts_with("gpu.") => return,
        Err(error) => panic!("{}", error.code()),
    };
    assert_eq!(rendered.objects, 1);
    let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
    let (width, height) = image.dimensions();
    let dark = |x: u32, y: u32| {
        let p = image.get_pixel(x, y).0;
        p[3] > 0 && p[0] < 128 && p[1] < 128 && p[2] < 128
    };
    let ink: Vec<(u32, u32)> = (0..width)
        .flat_map(|x| (0..height).map(move |y| (x, y)))
        .filter(|&(x, y)| dark(x, y))
        .collect();
    assert!(!ink.is_empty(), "the arc is drawn");
    // Framed by the exact bounds, the whole sweep is inside the image with a margin.
    assert!(
        ink.iter()
            .all(|&(x, y)| x > 0 && y > 0 && x + 1 < width && y + 1 < height),
        "the arc reaches the border: the frame ignored part of it"
    );
}
