//! Layouts and viewports edited by commands with their rules, named styles through the
//! dependency graph and the deletion rule, and purge against a reference-count model.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use std::collections::BTreeSet;

use arc_base::ObjectId;
use arc_doc::Value as DocValue;
use arc_doc::graph::StateKind;
use arc_draft::purge::{purgeable, unused};
use arc_draft::style::{Effective, effective_node};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::Fixture;

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn code(result: arc_base::Result<Value>) -> &'static str {
    result.unwrap_err().code()
}

fn layout(f: &mut Fixture, name: &str) -> (ObjectId, ObjectId) {
    let v = f
        .run(
            "core.draft.layout.create",
            json!({"name": name, "paper_width": 210.0, "paper_height": 297.0}),
        )
        .unwrap();
    let id = |k: &str| v[k].as_str().unwrap().parse().unwrap();
    (id("space"), id("layout"))
}

#[test]
fn layouts_and_viewports_follow_their_rules() {
    let mut f = Fixture::new();
    let model = f.space("model");
    let (sheet, settings) = layout(&mut f, "sheet");
    let set = |field: &str, value: Value| json!({"objects": [s(settings)], "field": field, "value": value});
    f.run(
        "core.draft.layout.modify",
        set("orientation", json!("landscape")),
    )
    .unwrap();
    f.run("core.draft.layout.modify", set("margin_left", json!(150.0)))
        .unwrap();
    // 150 + 150 leaves nothing of a 297 wide landscape sheet.
    assert_eq!(
        code(f.run(
            "core.draft.layout.modify",
            set("margin_right", json!(150.0))
        )),
        "draft.layout.margins_invalid"
    );
    assert_eq!(
        code(f.run(
            "core.draft.layout.modify",
            set("plot_area", json!("window"))
        )),
        "draft.layout.window_invalid"
    );
    assert_eq!(
        code(f.run("core.draft.layout.modify", set("paper_width", json!(0.0)))),
        "doc.value.out_of_range"
    );
    let viewport = |owner: ObjectId| json!({"layout": s(owner), "model": s(model), "center": [50.0, 50.0], "width": 40.0, "height": 30.0});
    assert_eq!(
        code(f.run("core.draft.viewport.create", viewport(model))),
        "draft.layout.owner_invalid"
    );
    let vp = f.create("core.draft.viewport.create", viewport(sheet));
    let open = json!({"objects": [s(vp)], "field": "clip",
        "value": {"kind": "segment", "start": [0.0, 0.0], "end": [1.0, 0.0]}});
    assert_eq!(
        code(f.run("core.draft.viewport.modify", open)),
        "draft.viewport.clip_invalid"
    );
    // A layout space turned into a model space would own viewports.
    assert_eq!(
        code(f.run(
            "core.draft.space.modify",
            json!({"objects": [s(sheet)], "field": "kind", "value": "model"})
        )),
        "draft.layout.owner_invalid"
    );
    f.run("core.draft.viewport.delete", json!({"objects": [s(vp)]}))
        .unwrap();
    assert!(f.document.snapshot().resolve(vp).is_err());
}

#[test]
fn changing_a_style_marks_its_users_stale_and_a_used_style_cannot_be_deleted() {
    let mut f = Fixture::drafting();
    let created = f.run("core.draft.style.defaults", json!({})).unwrap();
    assert_eq!(created["created"].as_array().unwrap().len(), 4);
    let again = f.run("core.draft.style.defaults", json!({})).unwrap();
    assert!(again["created"].as_array().unwrap().is_empty());
    let text = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "notes"}),
    );
    let dim = f.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "plans"}),
    );
    assert_eq!(
        code(f.run(
            "core.draft.style.create",
            json!({"kind": "text", "name": "notes"})
        )),
        "draft.name.taken"
    );
    f.set(
        "core.draft.style.modify",
        &[dim],
        "text_style",
        json!(s(text)),
    );
    let node = effective_node(dim).unwrap();
    let height = |f: &Fixture| {
        let view = f.document.graph().evaluate_now(&node).unwrap();
        let data = view.result.unwrap();
        let data = data.downcast_ref::<Effective>().unwrap();
        data.referenced[0].1.values[2].clone()
    };
    assert_eq!(height(&f), Some(DocValue::F64(0.0)));
    f.set("core.draft.style.modify", &[text], "height", json!(3.5));
    assert_eq!(
        f.document.graph().node(&node).unwrap().state.kind(),
        StateKind::Dirty
    );
    assert_eq!(height(&f), Some(DocValue::F64(3.5)));
    // The deletion rule refuses a style in use, and purge lists it as used.
    assert_eq!(
        code(f.run("core.draft.style.delete", json!({"objects": [s(text)]}))),
        "doc.reference.delete_blocked"
    );
    assert!(!unused(&f.document.snapshot()).contains(&text));
    // Parents: a child refines a style for all kinds.
    let child = f.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "radial"}),
    );
    let parent = json!({"objects": [s(child)], "field": "parent", "value": s(dim)});
    assert_eq!(
        code(f.run("core.draft.style.modify", parent.clone())),
        "draft.style.parent_invalid"
    );
    f.set("core.draft.style.modify", &[child], "kind", json!("radial"));
    f.run("core.draft.style.modify", parent).unwrap();
    f.run("core.draft.style.delete", json!({"objects": [s(child)]}))
        .unwrap();
}

/// Every reference a value holds, at any depth.
fn references(value: &DocValue, out: &mut BTreeSet<ObjectId>) {
    match value {
        DocValue::Reference(id) => {
            out.insert(*id);
        }
        DocValue::List(items) => items.iter().for_each(|v| references(v, out)),
        DocValue::Record(fields) => fields.iter().flatten().for_each(|v| references(v, out)),
        DocValue::Map(map) => map.values().for_each(|v| references(v, out)),
        _ => {}
    }
}

/// The reference-count model: purgeable objects that no field of any live object names.
fn model_unused(f: &Fixture) -> Vec<ObjectId> {
    let snapshot = f.document.snapshot();
    let mut used = BTreeSet::new();
    for (_, object) in snapshot.iter() {
        object
            .values
            .iter()
            .flatten()
            .for_each(|v| references(v, &mut used));
    }
    let keys = purgeable();
    let mut out: Vec<ObjectId> = snapshot
        .iter()
        .filter(|(h, _)| keys.contains(&snapshot.type_key(*h).unwrap().as_str()))
        .map(|(_, o)| o.id)
        .filter(|id| !used.contains(id))
        .collect();
    out.sort();
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

    #[test]
    fn purge_lists_exactly_the_unreferenced_objects_and_removes_them_in_one_undoable_step(
        uses in prop::collection::vec((0_u8..6, 0_usize..16, 0_usize..16), 0..12),
    ) {
        let mut f = Fixture::new();
        let space = f.space("model");
        let mut layers = Vec::new();
        let mut linetypes = Vec::new();
        let mut blocks = Vec::new();
        let mut texts = Vec::new();
        for i in 0..3 {
            layers.push(f.create("core.draft.layer.create", json!({"name": format!("l{i}")})));
            linetypes.push(f.create(
                "core.draft.linetype.load",
                json!({"name": format!("t{i}"), "elements": [{"kind": "dash", "length": 1.0}]}),
            ));
            let b = f.create("core.draft.block.define", json!({"name": format!("b{i}")}));
            f.line(b);
            blocks.push(b);
            texts.push(f.create("core.draft.style.create", json!({"kind": "text", "name": format!("x{i}")})));
        }
        let dims: Vec<ObjectId> = (0..2)
            .map(|i| f.create("core.draft.style.create", json!({"kind": "dimension", "name": format!("d{i}")})))
            .collect();
        for (kind, a, b) in uses {
            let line = || json!({"kind": "segment", "start": [0.0, 0.0], "end": [1.0, 0.0]});
            match kind {
                0 => {
                    let e = f.entity(space, line());
                    f.set("core.draft.entity.modify", &[e], "layer", json!(s(layers[a % 3])));
                }
                1 => f.set("core.draft.layer.modify", &[layers[a % 3]], "linetype", json!(s(linetypes[b % 3]))),
                2 => {
                    // References between blocks only go one way, so no cycle forms.
                    let (outer, inner) = (a % 3, b % 3);
                    if outer < inner {
                        f.run("core.draft.block.insert", json!({"owner": s(blocks[outer]), "block": s(blocks[inner]), "position": [0.0, 0.0]})).unwrap();
                    }
                }
                3 => {
                    f.run("core.draft.block.insert", json!({"owner": s(space), "block": s(blocks[a % 3]), "position": [0.0, 0.0]})).unwrap();
                }
                4 => f.set("core.draft.style.modify", &[dims[a % 2]], "text_style", json!(s(texts[b % 3]))),
                _ => {
                    let e = f.entity(blocks[a % 3], line());
                    f.set("core.draft.entity.modify", &[e], "linetype", json!(s(linetypes[b % 3])));
                }
            }
        }
        let listed = unused(&f.document.snapshot());
        prop_assert_eq!(&listed, &model_unused(&f));
        let before = f.document.snapshot().len();
        let ids: Vec<String> = listed.iter().map(ToString::to_string).collect();
        if !ids.is_empty() {
            f.run("core.draft.purge", json!({"objects": ids})).unwrap();
            let after = f.document.snapshot();
            prop_assert!(listed.iter().all(|id| after.resolve(*id).is_err()));
            prop_assert!(f.document.undo().wait().unwrap().value);
            prop_assert_eq!(f.document.snapshot().len(), before);
            prop_assert_eq!(&unused(&f.document.snapshot()), &listed);
        }
        // A used object is refused and nothing is removed.
        let used: Vec<ObjectId> = [layers.clone(), linetypes.clone(), blocks.clone(), texts.clone(), dims.clone()]
            .concat()
            .into_iter()
            .filter(|id| !listed.contains(id))
            .collect();
        if let Some(first) = used.first() {
            let mut all = vec![s(*first)];
            all.extend(listed.iter().map(ToString::to_string));
            prop_assert_eq!(code(f.run("core.draft.purge", json!({"objects": all}))), "draft.purge.in_use");
            prop_assert_eq!(f.document.snapshot().len(), before);
        }
    }
}
