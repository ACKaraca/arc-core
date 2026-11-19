//! The import command through the registry and invoker: transactions, units, loss reports,
//! limits, cancellation, file access and surfaces.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use arc_base::{Category, ObjectId};
use arc_command::{Invocation, Permission, Surface};
use arc_io_dxf::model::{Hatch, Insert, MText, Text};
use arc_io_dxf::{Entity, EntityKind, Value as GroupValue};
use serde_json::{Value, json};
use support::{
    Env, bytes, cancelled, circle, drawing, limits_with_records, line, on_layer, with_handle,
};

fn report_reasons(result: &Value) -> Vec<(String, Option<String>, Option<String>)> {
    result["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["reason"].as_str().unwrap().to_owned(),
                i["record_kind"].as_str().map(str::to_owned),
                i["source_id"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn keys(env: &Env, space: ObjectId) -> Vec<String> {
    env.exported(space)
        .entities
        .iter()
        .map(|e| e.kind.type_name().to_owned())
        .collect()
}

#[test]
fn an_import_is_one_transaction_and_one_undo_removes_it() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let before = env.snapshot();
    let path = env.write(
        "a.dxf",
        &bytes(&drawing(
            vec![
                on_layer(line([0.0; 3], [10.0, 0.0, 0.0]), "WALLS"),
                circle([5.0, 5.0, 0.0], 2.0),
                line([0.0; 3], [0.0, 10.0, 0.0]),
            ],
            Some(4),
        )),
    );
    let outcome = env.import(&path, space).unwrap();
    assert_eq!(outcome.value["entities"], 3);
    assert_eq!(outcome.value["layers"], 2);
    assert_eq!(
        outcome.revision,
        before.revision() + 1,
        "one revision for the whole import"
    );
    assert!(outcome.txn_id.is_some());
    let after = env.snapshot();
    assert_eq!(after.len(), before.len() + 5);
    assert!(env.document.undo().wait().unwrap().value);
    let undone = env.snapshot();
    assert_eq!(undone.len(), before.len());
    assert!(env.document.redo().wait().unwrap().value);
    assert_eq!(env.snapshot().len(), after.len());
}

#[test]
fn a_file_in_inches_is_converted_into_a_millimeter_space() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(
        "inch.dxf",
        &bytes(&drawing(
            vec![
                line([0.0; 3], [1.0, 0.0, 0.0]),
                circle([2.0, 0.0, 0.0], 0.5),
            ],
            Some(1),
        )),
    );
    let result = env.import(&path, space).unwrap().value;
    let d = env.exported(space);
    let EntityKind::Line(l) = &d.entities[0].kind else {
        panic!("line")
    };
    assert!((l.end[0] - 25.4).abs() < 1e-9);
    let EntityKind::Circle(c) = &d.entities[1].kind else {
        panic!("circle")
    };
    assert!((c.radius - 12.7).abs() < 1e-9 && (c.center[0] - 50.8).abs() < 1e-9);
    assert!(
        report_reasons(&result)
            .iter()
            .any(|r| r.0 == "draft.dxf.units_converted")
    );
    // A unitless file is taken in the target's unit and reported.
    let space2 = env.space("model2", "millimeter");
    let path = env.write(
        "none.dxf",
        &bytes(&drawing(vec![line([0.0; 3], [1.0, 0.0, 0.0])], None)),
    );
    let result = env.import(&path, space2).unwrap().value;
    let EntityKind::Line(l) = &env.exported(space2).entities[0].kind else {
        panic!("line")
    };
    assert!((l.end[0] - 1.0).abs() < 1e-12);
    assert!(
        report_reasons(&result)
            .iter()
            .any(|r| r.0 == "draft.dxf.units_unitless")
    );
}

#[test]
fn unmapped_kinds_are_listed_with_handle_and_kind_and_the_rest_is_imported() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let solid = with_handle(
        Entity::new(EntityKind::Solid(arc_io_dxf::model::Quad::default())),
        0xAB,
    );
    let face = with_handle(
        Entity::new(EntityKind::Face(arc_io_dxf::model::Quad::default())),
        0xAC,
    );
    let path = env.write(
        "u.dxf",
        &bytes(&drawing(
            vec![
                line([0.0; 3], [1.0, 1.0, 0.0]),
                solid,
                circle([0.0; 3], 3.0),
                face,
            ],
            Some(4),
        )),
    );
    let result = env.import(&path, space).unwrap().value;
    assert_eq!(result["entities"], 2);
    let reasons = report_reasons(&result);
    assert!(reasons.contains(&(
        "draft.dxf.entity_unmapped".to_owned(),
        Some("SOLID".to_owned()),
        Some("AB".to_owned())
    )));
    assert!(reasons.contains(&(
        "draft.dxf.entity_unmapped".to_owned(),
        Some("3DFACE".to_owned()),
        Some("AC".to_owned())
    )));
    assert_eq!(result["report"]["header"]["format"], "dxf");
    assert!(result["report"]["summary"]["dropped"].as_u64().unwrap() >= 2);
}

#[test]
fn limits_and_cancellation_are_passed_through() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(
        "l.dxf",
        &bytes(&drawing(
            (0..20)
                .map(|i| line([0.0; 3], [f64::from(i) + 1.0, 0.0, 0.0]))
                .collect(),
            None,
        )),
    );
    env.session.limits = limits_with_records(5);
    let error = env.import(&path, space).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(error.category(), Category::LimitExceeded);
    env.session.limits = arc_base::Limits::DEFAULT;
    let before = env.snapshot().revision();
    let invocation = Invocation {
        cancel: cancelled(),
        ..Invocation::new(
            "core.draft.import.dxf",
            json!({"path": path, "space": space.to_string()}),
        )
    };
    let error = env.invoke_with(invocation).unwrap_err();
    assert_eq!(error.category(), Category::Cancelled);
    assert_eq!(
        env.snapshot().revision(),
        before,
        "a cancelled import changes nothing"
    );
    // The format crate's own token yields base.cancelled.
    let cancel = cancelled();
    let mut options = arc_io_dxf::ReadOptions {
        cancel,
        ..arc_io_dxf::ReadOptions::default()
    };
    let error = arc_io_dxf::read(&std::fs::read(&path).unwrap(), &mut options).unwrap_err();
    assert_eq!(error.code(), "base.cancelled");
}

#[test]
fn file_access_outside_the_roots_and_without_permission_is_denied() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let outside = std::env::temp_dir().join("arc-draft-dxf-outside.dxf");
    std::fs::write(&outside, bytes(&drawing(vec![], None))).unwrap();
    let error = env.import(outside.to_str().unwrap(), space).unwrap_err();
    std::fs::remove_file(&outside).unwrap();
    assert_eq!(error.code(), "cmd.path.outside_roots");
    let inside = env.write("in.dxf", &bytes(&drawing(vec![], None)));
    // A path that leaves the root through `..`; it names no file, so every platform refuses it.
    let traversal = format!("{}/../arc-draft-dxf-none.dxf", env.dir.0.display());
    assert!(env.import(&traversal, space).is_err());
    let error = env
        .invoke(
            "core.draft.export.dxf",
            json!({"path": outside.to_str().unwrap(), "space": space.to_string()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.path.outside_roots");
    assert!(!outside.exists());
    env.session.permissions.remove(&Permission::FileRead);
    let error = env.import(&inside, space).unwrap_err();
    assert_eq!(error.code(), "cmd.permission_denied");
    env.session.permissions.remove(&Permission::FileWrite);
    let error = env
        .invoke(
            "core.draft.export.dxf",
            json!({"path": env.path("x.dxf").to_str().unwrap(), "space": space.to_string()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.permission_denied");
}

#[test]
fn the_commands_are_available_on_every_surface() {
    for surface in Surface::ALL {
        let mut env = Env::new();
        env.session.surface = surface;
        let space = env.space("model", "meter");
        let path = env.write(
            "s.dxf",
            &bytes(&drawing(vec![line([0.0; 3], [1.0, 0.0, 0.0])], Some(6))),
        );
        assert_eq!(
            env.import(&path, space).unwrap().value["entities"],
            1,
            "{surface:?}"
        );
        let out = env.path("out.dxf");
        let result = env
            .invoke(
                "core.draft.export.dxf",
                json!({"path": out.to_str().unwrap(), "space": space.to_string()}),
            )
            .unwrap();
        assert_eq!(result.value["entities"], 1);
        assert!(out.exists());
    }
    let mut env = Env::new();
    let catalog = env.commands.catalog();
    let ids: Vec<&str> = catalog["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"core.draft.import.dxf") && ids.contains(&"core.draft.export.dxf"));
    let space = env.space("m", "meter");
    let error = env
        .invoke(
            "core.draft.import.dxf",
            json!({"path": "x", "space": space.to_string(), "extra": 1}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.params.invalid");
}

#[test]
fn the_target_must_be_a_model_space() {
    let mut env = Env::new();
    let layout = env.create(
        "core.draft.space.create",
        json!({"name": "sheet", "kind": "layout"}),
    );
    let path = env.write("t.dxf", &bytes(&drawing(vec![], None)));
    assert_eq!(
        env.import(&path, layout).unwrap_err().code(),
        "draft.dxf.space_invalid"
    );
    let missing: ObjectId = arc_base::id::Identifier::from_u128(77);
    assert_eq!(
        env.import(&path, missing).unwrap_err().code(),
        "draft.dxf.space_invalid"
    );
}

#[test]
fn definitions_text_blocks_and_hatches_map_to_drafting_objects() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(vec![], Some(4));
    let mtext = Entity::new(EntityKind::MText(MText {
        insert: [1.0, 2.0, 0.0],
        height: 3.0,
        text: "{\\fArial|b1;Bold} one\\Ptwo \\S1/2;".to_owned(),
        ..MText::default()
    }));
    let text = Entity::new(EntityKind::Text(Text {
        insert: [0.0; 3],
        height: 2.0,
        value: "45%%d and %%c10".to_owned(),
        ..Text::default()
    }));
    let attdef = Entity::new(EntityKind::AttDef(arc_io_dxf::model::AttDef {
        tag: "ID".to_owned(),
        prompt: "Id".to_owned(),
        value: "none".to_owned(),
        height: 2.0,
        ..Default::default()
    }));
    d.blocks.push(support::block(
        "DOOR",
        vec![line([0.0; 3], [900.0, 0.0, 0.0]), attdef],
    ));
    let mut insert = Insert {
        block: "door".to_owned(),
        insert: [100.0, 100.0, 0.0],
        rotation: Some(90.0),
        x_scale: Some(2.0),
        ..Insert::default()
    };
    insert
        .attribs
        .push(Entity::new(EntityKind::Attrib(arc_io_dxf::model::Attrib {
            tag: "ID".to_owned(),
            value: "D-1".to_owned(),
            ..Default::default()
        })));
    insert
        .attribs
        .push(Entity::new(EntityKind::Attrib(arc_io_dxf::model::Attrib {
            tag: "OTHER".to_owned(),
            value: "x".to_owned(),
            ..Default::default()
        })));
    let hatch = Entity::new(EntityKind::Hatch(Hatch {
        pattern_name: "SOLID".to_owned(),
        solid: 1,
        loops: vec![arc_io_dxf::model::HatchLoop {
            flags: 3,
            boundary: arc_io_dxf::model::HatchBoundary::Polyline {
                has_bulge: false,
                closed: true,
                vertices: [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
                    .map(|location| arc_io_dxf::model::HatchVertex {
                        location,
                        bulge: 0.0,
                    })
                    .to_vec(),
            },
            sources: vec![],
        }],
        ..Hatch::default()
    }));
    d.entities = vec![mtext, text, Entity::new(EntityKind::Insert(insert)), hatch];
    let path = env.write("m.dxf", &bytes(&d));
    let result = env.import(&path, space).unwrap().value;
    assert_eq!(result["blocks"], 1);
    assert_eq!(
        result["entities"], 5,
        "text, rich text, reference, hatch and the block's line"
    );
    let reasons = report_reasons(&result);
    for expected in ["draft.dxf.text_formatting", "draft.dxf.attribute_dropped"] {
        assert!(reasons.iter().any(|r| r.0 == expected), "{expected}");
    }
    let out = env.exported(space);
    let kinds: Vec<&str> = out.entities.iter().map(|e| e.kind.type_name()).collect();
    assert_eq!(kinds, ["MTEXT", "TEXT", "INSERT", "HATCH"]);
    let EntityKind::MText(m) = &out.entities[0].kind else {
        panic!()
    };
    assert_eq!(m.text, "Bold one\\Ptwo 1/2");
    let EntityKind::Text(t) = &out.entities[1].kind else {
        panic!()
    };
    assert_eq!(t.value, "45\u{b0} and \u{2300}10");
    let EntityKind::Insert(i) = &out.entities[2].kind else {
        panic!()
    };
    assert_eq!((i.block.as_str(), i.attribs.len()), ("DOOR", 1));
    assert_eq!(i.x_scale, Some(2.0));
    assert!(i.rotation.is_some_and(|r| (r - 90.0).abs() < 1e-9));
    let _ = GroupValue::Int(0);
}

#[test]
fn block_cycles_missing_blocks_and_array_inserts_are_handled() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(vec![], Some(4));
    let insert_of = |name: &str| {
        Entity::new(EntityKind::Insert(Insert {
            block: name.to_owned(),
            ..Insert::default()
        }))
    };
    d.blocks.push(support::block(
        "A",
        vec![insert_of("B"), line([0.0; 3], [1.0, 0.0, 0.0])],
    ));
    d.blocks
        .push(support::block("B", vec![insert_of("A"), insert_of("B")]));
    let array = Entity::new(EntityKind::Insert(Insert {
        block: "A".to_owned(),
        columns: Some(3),
        rows: Some(2),
        column_spacing: Some(10.0),
        row_spacing: Some(5.0),
        ..Insert::default()
    }));
    d.entities = vec![array, insert_of("missing")];
    let path = env.write("c.dxf", &bytes(&d));
    let result = env.import(&path, space).unwrap().value;
    let reasons = report_reasons(&result);
    assert!(reasons.iter().any(|r| r.0 == "draft.dxf.array_expanded"));
    assert_eq!(
        reasons
            .iter()
            .filter(|r| r.0 == "draft.dxf.reference_unresolved")
            .count(),
        3
    );
    assert_eq!(
        keys(&env, space).iter().filter(|k| *k == "INSERT").count(),
        6
    );
}

#[test]
fn two_imports_into_one_space_share_definitions_and_stack_in_draw_order() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let d = drawing(
        vec![on_layer(line([0.0; 3], [1.0, 0.0, 0.0]), "WALLS")],
        Some(4),
    );
    let path = env.write("twice.dxf", &bytes(&d));
    let first = env.import(&path, space).unwrap().value;
    let second = env.import(&path, space).unwrap().value;
    assert_eq!(first["layers"], 2);
    assert_eq!(second["layers"], 0, "layers with the same name are reused");
    assert_eq!(keys(&env, space), ["LINE", "LINE"]);
}
