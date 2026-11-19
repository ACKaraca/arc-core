//! Plots of placed external references (CORE-DRAFT-025): a viewport onto a model space with a
//! placed reference shows the linked drawing when links are loaded, and reports the problem
//! when they are not.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId, Result};
use arc_doc::Txn;
use arc_doc::link::{self, LinkSpec, LoadMode, Opened};
use arc_draft::display::{DisplayOptions, Lists};
use arc_draft::external::WorkBudget;
use arc_draft::plot::{
    PlotOptions, layout_stream, layout_stream_with_links, layout_stream_with_work_budget,
};
use serde_json::json;
use support::{Fixture, plot_scene, segment_json};

#[test]
fn a_placed_reference_in_a_viewported_model_is_plotted_with_its_links_and_reported_without() {
    let mut scene = plot_scene(Fixture::drafting_with_links());
    let display = DisplayOptions::default();
    let options = PlotOptions::from_layout(&scene.f.document.snapshot(), scene.sheet).unwrap();
    let baseline = layout_stream(
        &scene.f.document.snapshot(),
        Lists::Direct,
        scene.sheet,
        &options,
        &display,
    )
    .unwrap();

    // The linked drawing, in meters like the model: one line from (0, -3) to (10, -3).
    let mut linked = Fixture::drafting();
    let linked_space = linked.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "meter"}),
    );
    linked.entity(linked_space, segment_json(0.0, -3.0, 10.0, -3.0));
    let linked_snapshot = linked.document.snapshot();

    let link_id = ObjectId::from_u128(10);
    let linked_id = ObjectId::from_u128(2);
    let spec = LinkSpec {
        document: linked_id,
        path: "linked.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    scene
        .f
        .document
        .submit(move |txn: &mut Txn| link::create(txn, link_id, &spec))
        .wait()
        .unwrap();
    scene.f.create(
        "core.draft.external_ref.create",
        json!({"owner": scene.model.to_string(), "link": link_id.to_string(), "position": [0.0, 0.0]}),
    );

    let snapshot = scene.f.document.snapshot();
    let resolver = move |_: &str, _: &str| -> Result<Opened> {
        Ok(Opened {
            location: "linked.arc".to_owned(),
            document: linked_id,
            content: [7; 32],
            snapshot: linked_snapshot.clone(),
        })
    };
    let links = link::load(
        &snapshot,
        ObjectId::from_u128(1),
        "host.arc",
        &resolver,
        &Limits::DEFAULT,
    );

    let with = layout_stream_with_links(
        &snapshot,
        Lists::Direct,
        Some(&links),
        scene.sheet,
        &options,
        &display,
    )
    .unwrap();
    assert!(with.problems.is_empty(), "{:?}", with.problems);
    assert_eq!(
        with.stream.items.len(),
        baseline.items.len() + 1,
        "the linked line is plotted in addition to the model's lines"
    );

    let without = layout_stream_with_links(
        &snapshot,
        Lists::Direct,
        None,
        scene.sheet,
        &options,
        &display,
    )
    .unwrap();
    assert_eq!(without.stream.items.len(), baseline.items.len());
    assert_eq!(without.problems.len(), 1, "{:?}", without.problems);
    assert_eq!(without.problems[0].code, "draft.external_ref.no_links");
    assert_eq!(
        without.stream.failed.len(),
        1,
        "the placed reference is left out"
    );
    // The plain entry point keeps its signature and result.
    let plain = layout_stream(&snapshot, Lists::Direct, scene.sheet, &options, &display).unwrap();
    assert_eq!(plain, without.stream);
}

#[test]
fn viewports_of_one_layout_share_one_build_of_a_linked_space() {
    let mut scene = plot_scene(Fixture::drafting_with_links());
    for center in [[50.0, 150.0], [50.0, 60.0]] {
        scene.f.create(
            "core.draft.viewport.create",
            json!({"layout": scene.sheet.to_string(), "model": scene.model.to_string(),
                   "center": center, "width": 60.0, "height": 40.0,
                   "view_center": [5.0, 0.0], "scale": 0.01}),
        );
    }
    let mut linked = Fixture::drafting();
    let linked_space = linked.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "meter"}),
    );
    linked.entity(linked_space, segment_json(0.0, -3.0, 10.0, -3.0));
    let linked_snapshot = linked.document.snapshot();

    let link_id = ObjectId::from_u128(10);
    let linked_id = ObjectId::from_u128(2);
    let spec = LinkSpec {
        document: linked_id,
        path: "linked.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    scene
        .f
        .document
        .submit(move |txn: &mut Txn| link::create(txn, link_id, &spec))
        .wait()
        .unwrap();
    scene.f.create(
        "core.draft.external_ref.create",
        json!({"owner": scene.model.to_string(), "link": link_id.to_string(), "position": [0.0, 0.0]}),
    );
    let snapshot = scene.f.document.snapshot();
    let resolver = move |_: &str, _: &str| -> Result<Opened> {
        Ok(Opened {
            location: "linked.arc".to_owned(),
            document: linked_id,
            content: [7; 32],
            snapshot: linked_snapshot.clone(),
        })
    };
    let links = link::load(
        &snapshot,
        ObjectId::from_u128(1),
        "host.arc",
        &resolver,
        &Limits::DEFAULT,
    );
    let options = PlotOptions::from_layout(&snapshot, scene.sheet).unwrap();
    // One line: the build holds 1 primitive and each of the three viewports copies it.
    let budget = WorkBudget {
        primitives: 4,
        ..WorkBudget::default()
    };
    let (plot, used) = layout_stream_with_work_budget(
        &snapshot,
        Lists::Direct,
        Some(&links),
        scene.sheet,
        &options,
        &DisplayOptions::default(),
        budget,
    )
    .unwrap();
    assert!(plot.problems.is_empty(), "{:?}", plot.problems);
    assert_eq!(
        used.expansions, 1,
        "the linked space is built once for all viewports"
    );
    assert_eq!(used.primitives, 4, "one build and three copies");
}
