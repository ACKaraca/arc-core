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

use arc_base::Limits;
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
    let before = f.document.snapshot();
    assert_eq!(before.len(), 16);

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
