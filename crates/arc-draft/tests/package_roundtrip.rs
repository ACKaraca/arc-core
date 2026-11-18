//! A document holding every drafting type, saved to a package and opened again, has the same
//! objects: identifiers, type keys, headers and field values.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use std::sync::Arc;

use arc_base::{Limits, ObjectId};
use arc_doc::graph::Evaluators;
use arc_doc::package::{LockOwner, Metadata, Mode, Package};
use arc_doc::{DocumentConfig, LoadOptions, Snapshot};
use serde_json::json;
use support::{Fixture, curve_json, every_curve, types};

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

/// Adds a layout with settings, a viewport with overrides and a clip, a wipeout and the default
/// styles with a table row format (CORE-DRAFT-004, 019, 027, 028).
fn add_sheet_objects(f: &mut Fixture, space: ObjectId, layer: ObjectId, dashed: ObjectId) {
    let created = f
        .run(
            "core.draft.layout.create",
            json!({"name": "sheet 2", "unit": "inch", "paper_width": 8.5, "paper_height": 11.0, "orientation": "landscape"}),
        )
        .unwrap();
    let sheet = created["space"].as_str().unwrap().to_owned();
    let settings = created["layout"].as_str().unwrap().to_owned();
    f.run(
        "core.draft.layout.modify",
        json!({"objects": [settings], "field": "plot_style_table", "value": "monochrome"}),
    )
    .unwrap();
    let viewport = f.create(
        "core.draft.viewport.create",
        json!({"layout": sheet, "model": space.to_string(), "center": [5.0, 4.0], "width": 6.0,
               "height": 4.0, "view_center": [1.0, 1.0], "scale": 0.02, "rotation": 0.25}),
    );
    let square = json!({"kind": "polyline", "closed": true, "vertices": [
        {"point": [0.0, 0.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [2.0, 0.0], "bulge": 0.3, "start_width": 0.0, "end_width": 0.0},
        {"point": [2.0, 2.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}]});
    for (field, value) in [
        (
            "layer_overrides",
            json!([[
                layer.to_string(),
                true,
                [0.0, 0.0, 1.0, 1.0],
                dashed.to_string(),
                0.3,
                0.5
            ]]),
        ),
        ("clip", square.clone()),
        ("locked", json!(true)),
        ("annotation_scale", json!(0.01)),
    ] {
        f.run(
            "core.draft.viewport.modify",
            json!({"objects": [viewport.to_string()], "field": field, "value": value}),
        )
        .unwrap();
    }
    f.run(
        "core.draft.wipeout.create",
        json!({"owner": space.to_string(), "boundary": square, "frame": true}),
    )
    .unwrap();
    let styles = f.run("core.draft.style.defaults", json!({})).unwrap();
    let text = styles["created"][0].as_str().unwrap().to_owned();
    let table = styles["created"][3].as_str().unwrap().to_owned();
    f.run(
        "core.draft.style.modify",
        json!({"objects": [table], "field": "header", "value": [text, 3.5, "left", false, "0.00"]}),
    )
    .unwrap();
}

#[test]
fn every_entity_kind_survives_save_and_reopen() {
    let mut f = Fixture::new();
    let space = f.space("model");
    f.create(
        "core.draft.space.create",
        json!({"name": "sheet", "kind": "layout"}),
    );
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [
            {"kind": "dash", "length": 1.0},
            {"kind": "text", "length": 0.5, "content": "GAS", "style": "standard", "scale": 0.2, "offset": [0.0, -0.1]},
            {"kind": "gap", "length": 0.25}
        ]}),
    );
    let layer = f.create(
        "core.draft.layer.create",
        json!({"name": "walls", "linetype": dashed.to_string(), "lineweight": 0.5}),
    );
    for curve in every_curve() {
        f.entity(space, curve_json(&curve));
    }
    for description in [
        json!({"control_points": [[0.0, 0.0], [1.0, 2.0], [3.0, 2.0], [4.0, 0.0]]}),
        json!({"fit_points": [[0.0, 0.0], [1.0, 2.0], [3.0, 3.0], [5.0, 1.0]]}),
    ] {
        let mut params = description;
        params["owner"] = json!(space.to_string());
        f.run("core.draft.spline.create", params).unwrap();
    }
    f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "mono", "kind": "color_mapped", "styles": [{"index": 1, "screening": 50.0}]}),
    );
    let point = f.entity(space, json!({"kind": "point", "position": [1.0, 2.0]}));
    f.run(
        "core.draft.entity.modify",
        json!({"objects": [point.to_string()], "field": "extrusion", "value": [0.0, 1.0, 0.0]}),
    )
    .unwrap();
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "door", "unit": "inch", "base_point": [1.0, 0.0], "attributes": [{"tag": "NO", "default": "1", "preset": true}]}),
    );
    f.line(block);
    f.run(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [5.0, 5.0],
               "scale": [2.0, -1.0, 1.0], "rotation": 0.5, "attributes": {"NO": "7"}, "layer": layer.to_string()}),
    )
    .unwrap();
    add_sheet_objects(&mut f, space, layer, dashed);
    let before = f.document.snapshot();
    assert_eq!(before.len(), 27);

    let dir = std::env::temp_dir().join(format!("arc-draft-roundtrip-{}", std::process::id()));
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
