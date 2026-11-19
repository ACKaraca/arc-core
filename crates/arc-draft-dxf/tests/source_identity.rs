//! Source identity through import and export: the handles of imported objects are bound in the
//! source map, the import lists what it created, the export keeps the bound handles on request,
//! duplicate and colliding handles are reported, and one undo removes the bindings.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Limits, ObjectId};
use arc_doc::SourceMap;
use arc_draft_dxf::export::export_drawing;
use arc_io_dxf::model::{
    Head, Insert, Linetype, LinetypeElement, Table, TableEntry, Text, TextStyle,
};
use arc_io_dxf::{Drawing, Entity, EntityKind, Group, Handle, ReadOptions};
use serde_json::{Value, json};
use support::{Env, block, bytes, circle, drawing, layer_entry, line, with_handle};

const SOURCE: &str = "in.dxf";

fn head(handle: u64) -> Head {
    Head {
        handle: Some(Handle(handle)),
        ..Head::default()
    }
}

fn with_head<T>(mut entry: TableEntry<T>, handle: u64) -> TableEntry<T> {
    entry.head = head(handle);
    entry
}

/// A drawing whose objects carry handles: a layer, a linetype, a text style and a block with
/// two entities, model-space entities of each kind, and a paper-space entity (not imported).
fn fixture() -> Drawing {
    let mut d = drawing(Vec::new(), Some(4));
    d.tables.layers = Some(Table {
        entries: vec![
            with_head(layer_entry("0", 7), 0x10),
            with_head(layer_entry("WALLS", 1), 0x11),
        ],
        ..Table::default()
    });
    d.tables.linetypes = Some(Table {
        entries: vec![with_head(
            TableEntry {
                name: "DASHED".to_owned(),
                data: Linetype {
                    description: "dashes".to_owned(),
                    alignment: 65,
                    pattern_length: 7.5,
                    elements: vec![
                        LinetypeElement {
                            length: 5.0,
                            ..LinetypeElement::default()
                        },
                        LinetypeElement {
                            length: -2.5,
                            ..LinetypeElement::default()
                        },
                    ],
                },
                ..TableEntry::default()
            },
            0x12,
        )],
        ..Table::default()
    });
    d.tables.styles = Some(Table {
        entries: vec![with_head(
            TableEntry {
                name: "NOTES".to_owned(),
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
    d.tables.block_records = Some(Table {
        entries: vec![with_head(
            TableEntry {
                name: "DOOR".to_owned(),
                ..TableEntry::default()
            },
            0x30,
        )],
        ..Table::default()
    });
    let mut door = block(
        "DOOR",
        vec![
            on_walls(with_handle(circle([0.0; 3], 1.0), 0x36)),
            on_walls(with_handle(line([0.0; 3], [1.0, 0.0, 0.0]), 0x35)),
        ],
    );
    door.begin = with_handle(door.begin, 0x34);
    let mut paper = block(
        "*Paper_Space",
        vec![with_handle(line([0.0; 3], [5.0, 0.0, 0.0]), 0x41)],
    );
    paper.begin = with_handle(paper.begin, 0x40);
    d.blocks = vec![door, paper];
    let mut in_paper = with_handle(line([0.0; 3], [9.0, 0.0, 0.0]), 0x54);
    in_paper.props.paper_space = true;
    d.entities = vec![
        on_walls(with_handle(line([0.0; 3], [10.0, 0.0, 0.0]), 0x50)),
        with_handle(text_entity("HELLO", [1.0, 2.0, 0.0]), 0x51),
        with_handle(
            Entity::new(EntityKind::Insert(Insert {
                block: "DOOR".to_owned(),
                insert: [10.0, 0.0, 0.0],
                ..Insert::default()
            })),
            0x52,
        ),
        on_walls(with_handle(circle([5.0, 5.0, 0.0], 2.0), 0x53)),
        in_paper,
    ];
    d
}

fn on_walls(mut e: Entity) -> Entity {
    e.props.layer = "WALLS".to_owned();
    e
}

fn text_entity(value: &str, at: [f64; 3]) -> Entity {
    Entity::new(EntityKind::Text(Text {
        insert: at,
        height: 2.5,
        value: value.to_owned(),
        style: Some("NOTES".to_owned()),
        ..Text::default()
    }))
}

/// The drawing with its last entity's handle set to `handle`.
fn with_handle_of(mut d: Drawing, handle: u64) -> Drawing {
    if let Some(last) = d.entities.last_mut() {
        last.head.handle = Some(Handle(handle));
    }
    d
}

/// An import that lists the objects it created.
fn import_listing(env: &mut Env, path: &str, space: ObjectId) -> Value {
    env.run(
        "core.draft.import.dxf",
        json!({"path": path, "space": space.to_string(), "list_created": true}),
    )
}

/// The written file as a neutral drawing.
fn read_back(env: &Env, name: &str) -> Drawing {
    let data = std::fs::read(env.path(name)).unwrap();
    arc_io_dxf::read(&data, &mut ReadOptions::with_limits(Limits::DEFAULT))
        .unwrap()
        .drawing
}

fn export_keeping(env: &mut Env, space: ObjectId, name: &str, floor: u64) -> Value {
    let path = env.path(name).to_str().unwrap().to_owned();
    env.run(
        "core.draft.export.dxf",
        json!({
            "path": path,
            "space": space.to_string(),
            "keep_sources": {"document": SOURCE, "floor": floor},
        }),
    )
}

fn handles_of(result: &Value) -> BTreeMap<String, String> {
    result["handles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["object"].as_str().unwrap().to_owned(),
                h["handle"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn hex(h: &str) -> u64 {
    u64::from_str_radix(h, 16).unwrap()
}

/// The layer, linetype, style and block record of the fixture are written with their handles.
fn assert_tables_keep_handles(written: &Drawing) {
    let layer = |name: &str| {
        written
            .tables
            .layers
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|e| e.name == name)
            .and_then(|e| e.head.handle)
    };
    assert_eq!(layer("WALLS"), Some(Handle(0x11)));
    assert_eq!(
        written
            .tables
            .linetypes
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|e| e.name == "DASHED")
            .and_then(|e| e.head.handle),
        Some(Handle(0x12))
    );
    assert_eq!(
        written
            .tables
            .styles
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|e| e.name == "NOTES")
            .and_then(|e| e.head.handle),
        Some(Handle(0x13))
    );
    let door_record = written
        .tables
        .block_records
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|e| e.name == "DOOR")
        .and_then(|e| e.head.handle);
    assert_eq!(door_record, Some(Handle(0x30)));
}

#[test]
fn bound_handles_are_written_back_unchanged_and_the_export_lists_them() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&fixture()));
    let result = import_listing(&mut env, &path, space);

    let created = result["created"].as_array().unwrap();
    let bound: Vec<&str> = created
        .iter()
        .filter_map(|c| c["source"].as_str())
        .collect();
    for handle in ["11", "12", "13", "30", "35", "36", "50", "51", "52", "53"] {
        assert!(bound.contains(&handle), "{handle} is not bound: {bound:?}");
    }
    assert!(
        !bound.contains(&"54"),
        "a paper-space entity is not created"
    );
    let snapshot = env.snapshot();
    let map = SourceMap::of(&snapshot);
    let wall = created
        .iter()
        .find(|c| c["source"] == "11")
        .map(|c| c["object"].as_str().unwrap().to_owned())
        .unwrap();
    assert_eq!(
        map.resolve("dxf", SOURCE, "11").map(|id| id.to_string()),
        Some(wall.clone())
    );

    let out = export_keeping(&mut env, space, "out.dxf", 0);
    let written = read_back(&env, "out.dxf");
    assert_tables_keep_handles(&written);
    let door = written
        .blocks
        .iter()
        .find(|b| matches!(&b.begin.kind, EntityKind::Block(begin) if begin.name == "DOOR"))
        .unwrap();
    let door_handles: BTreeSet<u64> = door
        .entities
        .iter()
        .filter_map(|e| e.source_id())
        .map(|h| h.0)
        .collect();
    assert!(door_handles.is_superset(&BTreeSet::from([0x35, 0x36])));
    let model_handles: BTreeSet<u64> = written
        .entities
        .iter()
        .filter_map(|e| e.source_id())
        .map(|h| h.0)
        .collect();
    assert!(model_handles.is_superset(&BTreeSet::from([0x50, 0x51, 0x52, 0x53])));

    // Every bound handle is written with its own value, and the seed is above all of them.
    let highest = written
        .entities
        .iter()
        .chain(door.entities.iter())
        .filter_map(|e| e.source_id())
        .map(|h| h.0)
        .max()
        .unwrap();
    let seed = written.header_variable("$HANDSEED").unwrap();
    let Group { value, .. } = &seed[0];
    let arc_io_dxf::Value::Text(seed) = value else {
        panic!("seed is text")
    };
    assert!(u64::from_str_radix(seed, 16).unwrap() > highest);

    // The export lists the handle of every object it wrote; the bound ones keep theirs.
    let handles = handles_of(&out);
    assert_eq!(handles[&wall], "11");
    assert!(
        out["report"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["reason"] != "io.dxf.handle_assigned"),
        "the writer assigned no handle"
    );
}

#[test]
fn an_object_added_after_the_import_gets_a_handle_above_the_floor_and_all_others() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&fixture()));
    env.import(&path, space).unwrap();
    let added = env.create(
        "core.draft.entity.create",
        json!({
            "owner": space.to_string(),
            "geometry": serde_json::to_value(arc_geom::Curve2::Segment(
                arc_geom::Segment2::new(
                    arc_base::math::Point2::new(0.0, 0.0),
                    arc_base::math::Point2::new(3.0, 4.0),
                )
                .unwrap(),
            ))
            .unwrap(),
        }),
    );
    let result = export_keeping(&mut env, space, "out.dxf", 0x1000);
    let handles = handles_of(&result);
    let new = hex(&handles[&added.to_string()]);
    assert!(new > 0x1000, "{new:#x} is not above the floor");
    for (object, handle) in &handles {
        if *object != added.to_string() {
            assert!(hex(handle) < new, "{handle} is not below the new handle");
        }
    }
    let bound: BTreeSet<&String> = handles.values().collect();
    assert_eq!(bound.len(), handles.len(), "no two objects share a handle");
    assert_eq!(handles.values().filter(|h| **h == "11").count(), 1);
}

#[test]
fn a_handle_that_occurs_twice_binds_the_first_and_reports_the_second() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(
        vec![
            with_handle(line([0.0; 3], [1.0, 0.0, 0.0]), 0x60),
            with_handle(circle([0.0; 3], 3.0), 0x77),
        ],
        Some(4),
    );
    d.tables.layers = Some(Table::default());
    // The writer gives a repeated handle a new one, so the repeat is made in the text.
    let text = String::from_utf8(bytes(&with_handle_of(d, 0x77))).unwrap();
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let from = format!("  5{nl}77{nl}");
    assert_eq!(text.matches(&from).count(), 1, "the handle of the circle");
    let text = text.replace(&from, &format!("  5{nl}60{nl}"));
    let path = env.write(SOURCE, text.as_bytes());
    let result = import_listing(&mut env, &path, space);
    let created = result["created"].as_array().unwrap();
    let sources: Vec<&Value> = created.iter().map(|c| &c["source"]).collect();
    assert_eq!(sources.iter().filter(|s| ***s == json!("60")).count(), 1);
    let unbound = created
        .iter()
        .find(|c| c["source"].is_null() && c["kind"] == "core.draft.circle")
        .expect("the second circle is created without a source");
    assert_eq!(unbound["kind"], "core.draft.circle");
    let reasons: Vec<&str> = result["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect();
    assert!(
        reasons.contains(&"draft.dxf.handle_duplicate"),
        "{reasons:?}"
    );

    let out = export_keeping(&mut env, space, "out.dxf", 0);
    let handles = handles_of(&out);
    let first = created.iter().find(|c| c["source"] == "60").unwrap()["object"]
        .as_str()
        .unwrap();
    assert_eq!(handles[first], "60");
    let second = unbound["object"].as_str().unwrap();
    assert_ne!(handles[second], "60");
    assert!(hex(&handles[second]) > 0x60);
}

#[test]
fn one_undo_removes_the_import_and_its_bindings() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&fixture()));
    let before = env.snapshot();
    env.import(&path, space).unwrap();
    assert!(
        env.snapshot()
            .foreign_rows()
            .any(|r| r.record_kind == "source_map")
    );
    assert!(env.document.undo().wait().unwrap().value);
    let after = env.snapshot();
    assert_eq!(after.len(), before.len());
    assert!(after.foreign_rows().all(|r| r.record_kind != "source_map"));
    assert!(SourceMap::of(&after).resolve("dxf", SOURCE, "11").is_none());
}

#[test]
fn a_bound_handle_of_the_root_dictionary_moves_the_root_and_is_reported() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(
        SOURCE,
        &bytes(&drawing(
            vec![with_handle(line([0.0; 3], [1.0, 0.0, 0.0]), 0xC)],
            Some(4),
        )),
    );
    env.import(&path, space).unwrap();
    let out = export_keeping(&mut env, space, "out.dxf", 0);
    assert_eq!(handles_of(&out).values().filter(|h| *h == "C").count(), 1);
    let reasons: Vec<&str> = out["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect();
    assert!(
        reasons.contains(&"draft.dxf.handle_reassigned"),
        "{reasons:?}"
    );
    let written = read_back(&env, "out.dxf");
    assert!(
        written
            .entities
            .iter()
            .any(|e| e.source_id() == Some(Handle(0xC)))
    );
}

#[test]
fn the_default_export_assigns_every_handle() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&fixture()));
    env.import(&path, space).unwrap();
    let exported = export_drawing(&env.snapshot(), space).unwrap();
    let handles: Vec<Handle> = exported.handles.iter().map(|(_, h)| *h).collect();
    assert!(!handles.is_empty());
    assert!(
        handles.iter().all(|h| h.0 >= 0x20),
        "the default assigns above the structure"
    );
}

#[test]
fn a_mapped_object_that_shares_a_preserved_handle_is_not_bound_and_is_reported() {
    use arc_io_dxf::PreservedRecord;
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let record = PreservedRecord {
        kind: "HELIX".to_owned(),
        groups: vec![
            Group::text(5, "71"),
            Group::text(330, "1F"),
            Group::text(100, "AcDbEntity"),
            Group::text(8, "0"),
        ],
    };
    let mut d = drawing(
        vec![
            with_handle(line([0.0; 3], [1.0, 0.0, 0.0]), 0x70),
            Entity::new(EntityKind::Preserved(record)),
        ],
        Some(4),
    );
    d.tables.layers = Some(Table::default());
    // The writer would renumber a repeated handle, so the record's handle is changed in the text.
    let text = String::from_utf8(bytes(&d)).unwrap();
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let from = format!("  5{nl}71{nl}");
    assert_eq!(text.matches(&from).count(), 1, "the handle of the record");
    let text = text.replace(&from, &format!("  5{nl}70{nl}"));
    let path = env.write(SOURCE, text.as_bytes());
    let result = import_listing(&mut env, &path, space);
    let line_created = result["created"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "core.draft.line")
        .unwrap();
    assert!(line_created["source"].is_null(), "{line_created:?}");
    let reasons: Vec<&str> = result["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect();
    assert!(
        reasons.contains(&"draft.dxf.handle_duplicate"),
        "{reasons:?}"
    );
    let out = export_keeping(&mut env, space, "out.dxf", 0);
    let written = read_back(&env, "out.dxf");
    let handles: Vec<u64> = written
        .entities
        .iter()
        .filter_map(|e| e.source_id())
        .map(|h| h.0)
        .collect();
    assert_eq!(
        handles.iter().filter(|h| **h == 0x70).count(),
        1,
        "{handles:?}"
    );
    assert!(!reasons_of(&out).contains(&"draft.dxf.handle_reassigned"));
}

fn reasons_of(out: &Value) -> Vec<&str> {
    out["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["reason"].as_str())
        .collect()
}

#[test]
fn importing_the_same_file_again_binds_the_handles_to_the_new_objects() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(SOURCE, &bytes(&fixture()));
    let first = import_listing(&mut env, &path, space);
    let second = import_listing(&mut env, &path, space);
    let object_of = |result: &Value, handle: &str| {
        result["created"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["source"] == handle)
            .map(|c| c["object"].as_str().unwrap().to_owned())
            .unwrap()
    };
    let snapshot = env.snapshot();
    let map = SourceMap::of(&snapshot);
    let bound = map.resolve("dxf", SOURCE, "50").unwrap().to_string();
    assert_eq!(bound, object_of(&second, "50"));
    assert_ne!(bound, object_of(&first, "50"));
    // The earlier line is no longer bound, so the export gives it a new handle and writes the
    // bound one once.
    let out = export_keeping(&mut env, space, "out.dxf", 0);
    let handles = handles_of(&out);
    assert_eq!(handles[&bound], "50");
    assert_ne!(handles[&object_of(&first, "50")], "50");
}
