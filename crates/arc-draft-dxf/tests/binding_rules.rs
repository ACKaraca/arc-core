//! The import lists created objects only on request, reports the handles it moves between
//! imports of the same file, binds existing definitions to the file's handles, and the export
//! finds a definition by name without regard to case.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::ObjectId;
use arc_doc::SourceMap;
use arc_io_dxf::model::{Table, TableEntry, TextStyle};
use arc_io_dxf::{Drawing, Handle, ReadOptions};
use serde_json::{Value, json};
use support::{Env, bytes, drawing, layer_entry, line, with_handle};

const SOURCE: &str = "in.dxf";

fn with_head<T>(mut entry: TableEntry<T>, handle: u64) -> TableEntry<T> {
    entry.head.handle = Some(Handle(handle));
    entry
}

fn import(env: &mut Env, path: &str, space: ObjectId, list: Option<bool>) -> Value {
    let mut params = json!({"path": path, "space": space.to_string()});
    if let Some(list) = list {
        params["list_created"] = json!(list);
    }
    env.run("core.draft.import.dxf", params)
}

fn reasons(result: &Value) -> Vec<&str> {
    result["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect()
}

fn walls_drawing() -> Drawing {
    let mut d = drawing(
        vec![with_handle(line([0.0; 3], [1.0, 0.0, 0.0]), 0x50)],
        Some(4),
    );
    d.tables.layers = Some(Table {
        entries: vec![with_head(layer_entry("WALLS", 1), 0x11)],
        ..Table::default()
    });
    d
}

#[test]
fn the_created_objects_are_listed_only_when_requested() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&walls_drawing()));
    let plain = import(&mut env, &path, space, None);
    assert!(plain.get("created").is_none(), "{plain}");
    assert!(plain["entities"].as_u64().unwrap() >= 1);
    let listed = import(&mut env, &path, space, Some(true));
    assert!(!listed["created"].as_array().unwrap().is_empty());
}

#[test]
fn a_second_import_of_the_same_file_reports_the_handles_it_moves() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&walls_drawing()));
    let first = import(&mut env, &path, space, Some(true));
    assert!(!reasons(&first).contains(&"draft.dxf.binding_moved"));
    let second = import(&mut env, &path, space, Some(true));
    assert!(
        reasons(&second).contains(&"draft.dxf.binding_moved"),
        "{:?}",
        reasons(&second)
    );
}

#[test]
fn an_existing_definition_with_the_file_name_is_bound_to_the_file_handle() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let existing = env.create("core.draft.layer.create", json!({"name": "walls"}));
    let path = env.write(SOURCE, &bytes(&walls_drawing()));
    import(&mut env, &path, space, None);
    let snapshot = env.snapshot();
    let map = SourceMap::of(&snapshot);
    assert_eq!(map.resolve("dxf", SOURCE, "11"), Some(existing));
}

#[test]
fn a_style_matched_by_name_without_regard_to_case_keeps_its_bound_handle() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(Vec::new(), Some(4));
    d.tables.styles = Some(Table {
        entries: vec![with_head(
            TableEntry {
                name: "STANDARD".to_owned(),
                data: TextStyle {
                    width_factor: 1.0,
                    font: "txt".to_owned(),
                    ..TextStyle::default()
                },
                ..TableEntry::default()
            },
            0x13,
        )],
        ..Table::default()
    });
    let path = env.write(SOURCE, &bytes(&d));
    import(&mut env, &path, space, None);
    let path_out = env.path("out.dxf").to_str().unwrap().to_owned();
    env.run(
        "core.draft.export.dxf",
        json!({
            "path": path_out,
            "space": space.to_string(),
            "keep_sources": {"document": SOURCE},
        }),
    );
    let data = std::fs::read(env.path("out.dxf")).unwrap();
    let written: Drawing = arc_io_dxf::read(
        &data,
        &mut ReadOptions::with_limits(arc_base::Limits::DEFAULT),
    )
    .unwrap()
    .drawing;
    let handles: Vec<Option<Handle>> = written
        .tables
        .styles
        .unwrap()
        .entries
        .iter()
        .map(|e| e.head.handle)
        .collect();
    assert!(handles.contains(&Some(Handle(0x13))), "{handles:?}");
}
