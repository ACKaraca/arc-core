//! Arrays written to DXF: one block reference per item at the placement the display list draws,
//! the source block written as a block definition, the handle rule under `keep_sources`, the
//! array budget, and the loss item that records the associative definition as not kept.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result, Tolerance};
use arc_command::{CommandSpec, Context, Permission};
use arc_doc::SourceMap;
use arc_draft::array::{Expansion, expansion};
use arc_draft::display::{DisplayOptions, Lists, Shape, space_list};
use arc_draft_dxf::{HandlePolicy, export_drawing_limited};
use arc_geom::{Circle2, Curve2, Segment2};
use arc_io_dxf::loss::Argument;
use arc_io_dxf::model::Insert;
use arc_io_dxf::{Entity, EntityKind, ReadOptions, read};
use serde::Deserialize;
use serde_json::{Value, json};
use support::Env;

const TURN_TOLERANCE: f64 = 1e-12;

fn segment_json(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    serde_json::to_value(Curve2::Segment(
        Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap(),
    ))
    .unwrap()
}

fn circle_json(cx: f64, cy: f64, r: f64) -> Value {
    serde_json::to_value(Curve2::Circle(
        Circle2::new(Point2::new(cx, cy), r).unwrap(),
    ))
    .unwrap()
}

/// A block `B` whose content is the segment from (0, 0) to (1, 0), so that each item's display
/// segment starts at the item's position and points along the item's rotation.
fn unit_block(env: &mut Env, space: ObjectId) -> ObjectId {
    let line = env.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": segment_json(0.0, 0.0, 1.0, 0.0)}),
    );
    env.create(
        "core.draft.block.define",
        json!({"name": "B", "objects": [line.to_string()]}),
    )
}

fn array(env: &mut Env, owner: ObjectId, block: ObjectId, extra: Value) -> ObjectId {
    let mut params = json!({"owner": owner.to_string(), "block": block.to_string()});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    env.create("core.draft.array.create", params)
}

/// The expansion of the array as the display list places it (the same function the export uses).
fn expected(env: &Env, id: ObjectId) -> Expansion {
    expansion(&env.snapshot(), id, &Tolerance::DEFAULT).unwrap()
}

/// The display segments of `space`, in draw order: (start, end).
fn display_segments(env: &Env, space: ObjectId) -> Vec<(Point2, Point2)> {
    let snapshot = env.snapshot();
    let list = space_list(&snapshot, Lists::Direct, space, &DisplayOptions::default()).unwrap();
    list.primitives
        .iter()
        .filter_map(|p| match &p.shape {
            Shape::Stroke {
                curve: Curve2::Segment(s),
                ..
            } => Some((s.start(), s.end())),
            _ => None,
        })
        .collect()
}

/// The block references of a model space written to `name`, read back by the DXF reader, with the
/// layer of each reference. Fails when the file does not read.
fn written_inserts(env: &mut Env, space: ObjectId, name: &str) -> Vec<(String, Insert)> {
    let path = env.path(name);
    let path = path.to_str().unwrap().to_owned();
    env.run(
        "core.draft.export.dxf",
        json!({"path": path, "space": space.to_string(), "version": "2018"}),
    );
    let bytes = std::fs::read(&path).unwrap();
    let out = read(&bytes, &mut ReadOptions::with_limits(Limits::DEFAULT)).unwrap();
    out.drawing
        .entities
        .into_iter()
        .filter_map(insert_of)
        .collect()
}

fn insert_of(e: Entity) -> Option<(String, Insert)> {
    match e.kind {
        EntityKind::Insert(i) => Some((e.props.layer, i)),
        _ => None,
    }
}

/// The DXF reference placement of an INSERT as (insert point, rotation in radians, x and y scale).
fn placement(i: &Insert) -> (Point2, f64, f64, f64) {
    (
        Point2::new(i.insert[0], i.insert[1]),
        i.rotation.unwrap_or(0.0).to_radians(),
        i.x_scale.unwrap_or(1.0),
        i.y_scale.unwrap_or(1.0),
    )
}

/// The end of the block segment (1, 0) placed by an INSERT: the x scale, then the rotation, as
/// the DXF reference defines it (the segment has no y extent).
fn placed_end(start: Point2, rotation: f64, sx: f64) -> Point2 {
    let (x, y) = (sx, 0.0);
    let (s, c) = rotation.sin_cos();
    Point2::new(start.x + c * x - s * y, start.y + s * x + c * y)
}

/// Checks the written references against the expansion of the array (exact positions, rotation
/// within the stated tolerance) and the display segments of the drawing against the references.
/// `others` counts the segments drawn before the array's items that are not part of it (a path
/// entity the array follows is itself a segment).
fn assert_matches_display(env: &mut Env, space: ObjectId, id: ObjectId, file: &str, others: usize) {
    let exp = expected(env, id);
    let written = written_inserts(env, space, file);
    let inserts: Vec<_> = written.iter().map(|(_, i)| placement(i)).collect();
    assert_eq!(inserts.len(), exp.items.len(), "one reference per item");
    for ((at, rotation, sx, _), item) in inserts.iter().zip(&exp.items) {
        assert_eq!(*at, item.position, "position is exact");
        assert!(
            (rotation - item.rotation).abs() < TURN_TOLERANCE,
            "{rotation} != {}",
            item.rotation
        );
        assert_eq!(*sx, exp.scale.x);
    }
    let display: Vec<_> = display_segments(env, space)
        .into_iter()
        .skip(others)
        .collect();
    assert_eq!(
        display.len(),
        inserts.len(),
        "one displayed segment per item"
    );
    for ((start, end), (at, rotation, sx, _)) in display.iter().zip(&inserts) {
        assert_eq!(start, at, "display starts where the reference is inserted");
        let want = placed_end(*at, *rotation, *sx);
        assert!(
            (end.x - want.x).abs() < 1e-9 && (end.y - want.y).abs() < 1e-9,
            "display end {end:?} != reference end {want:?}"
        );
    }
}

fn space_with_block(env: &mut Env) -> (ObjectId, ObjectId) {
    let space = env.space("model", "millimeter");
    let block = unit_block(env, space);
    (space, block)
}

#[test]
fn a_rectangular_array_is_written_as_one_reference_per_item() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let a = array(
        &mut env,
        space,
        block,
        json!({"position": [5.0, -2.0], "rotation": 0.2,
               "array": {"kind": "rectangular", "rows": 2, "columns": 3,
                         "row_spacing": 4.0, "column_spacing": 7.5, "angle": 0.3}}),
    );
    assert_matches_display(&mut env, space, a, "rect.dxf", 0);
}

#[test]
fn a_polar_array_with_mirrored_items_is_written_mirrored() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let a = array(
        &mut env,
        space,
        block,
        json!({"position": [10.0, 0.0], "rotation": 0.1,
               "array": {"kind": "polar", "center": [0.0, 0.0], "count": 5,
                         "total_angle": 4.0, "rotate_items": true}}),
    );
    env.run(
        "core.draft.array.modify",
        json!({"objects": [a.to_string()], "field": "scale", "value": [-1.0, 1.0, 1.0]}),
    );
    assert_matches_display(&mut env, space, a, "polar.dxf", 0);
    let written = written_inserts(&mut env, space, "polar-mirror.dxf");
    assert!(written.iter().all(|(_, i)| i.x_scale == Some(-1.0)));
}

#[test]
fn a_path_array_along_an_arc_follows_the_tangent() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let path = env.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": circle_json(0.0, 0.0, 20.0)}),
    );
    let a = array(
        &mut env,
        space,
        block,
        json!({"position": [0.0, 0.0],
               "array": {"kind": "path", "path": path.to_string(), "count": 8, "align": true}}),
    );
    assert_matches_display(&mut env, space, a, "path.dxf", 0);
}

#[test]
fn a_path_array_with_spacing_on_a_segment_keeps_exact_distances() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let path = env.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": segment_json(0.0, 0.0, 10.0, 0.0)}),
    );
    let a = array(
        &mut env,
        space,
        block,
        json!({"position": [1.0, 1.0],
               "array": {"kind": "path", "path": path.to_string(), "spacing": 2.5, "align": false}}),
    );
    assert_matches_display(&mut env, space, a, "path-spacing.dxf", 1);
}

#[test]
fn the_source_block_is_written_and_each_array_is_reported_once() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let a = array(
        &mut env,
        space,
        block,
        json!({"array": {"kind": "rectangular", "rows": 3, "columns": 4,
                         "column_spacing": 3.0}}),
    );
    let exported = export_drawing_limited(
        &env.snapshot(),
        space,
        &HandlePolicy::Assign,
        &Limits::DEFAULT,
    )
    .unwrap();
    let names: Vec<_> = exported
        .drawing
        .blocks
        .iter()
        .map(|b| match &b.begin.kind {
            EntityKind::Block(begin) => begin.name.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(names.contains(&"B".to_owned()), "{names:?}");
    let reasons: Vec<_> = exported
        .report
        .items
        .iter()
        .filter(|i| i.reason == "draft.dxf.array_approximated")
        .collect();
    assert_eq!(reasons.len(), 1);
    assert_eq!(
        reasons[0].arguments.get("count"),
        Some(&Argument::Int(12)),
        "the item names the 12 references written"
    );
    assert!(
        exported
            .report
            .items
            .iter()
            .all(|i| i.reason != "draft.dxf.export_unmapped"),
        "{:?}",
        exported.report.items
    );
    assert!(exported.handles.iter().any(|(id, _)| *id == a));
}

#[test]
fn a_layer_and_an_array_inside_a_block_definition_are_written() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let walls = env.create("core.draft.layer.create", json!({"name": "walls"}));
    let inner = array(
        &mut env,
        space,
        block,
        json!({"layer": walls.to_string(),
               "array": {"kind": "rectangular", "rows": 1, "columns": 3, "column_spacing": 2.0}}),
    );
    env.create(
        "core.draft.block.define",
        json!({"name": "OUTER", "objects": [inner.to_string()]}),
    );
    let path = env.path("nested.dxf");
    let path = path.to_str().unwrap().to_owned();
    env.run(
        "core.draft.export.dxf",
        json!({"path": path, "space": space.to_string(), "version": "2018"}),
    );
    let bytes = std::fs::read(&path).unwrap();
    let out = read(&bytes, &mut ReadOptions::with_limits(Limits::DEFAULT)).unwrap();
    let outer = out
        .drawing
        .blocks
        .iter()
        .find(|b| matches!(&b.begin.kind, EntityKind::Block(x) if x.name == "OUTER"))
        .expect("the block that holds the array is written");
    let inserts: Vec<_> = outer
        .entities
        .iter()
        .filter_map(|e| insert_of(e.clone()))
        .collect();
    assert_eq!(inserts.len(), 3);
    assert!(
        inserts
            .iter()
            .all(|(layer, i)| layer == "walls" && i.block == "B")
    );
}

/// Binds the array `id` to the handle `handle` of the source document `model.dxf`, as an import
/// of that file would.
fn bind(env: &mut Env, id: ObjectId, handle: u64) {
    #[derive(Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Params {
        id: String,
        handle: String,
    }
    fn run(ctx: &mut Context<'_>, p: Params) -> Result<Value> {
        let id: ObjectId = p.id.parse().unwrap();
        let mut sources = SourceMap::of(ctx.snapshot());
        sources.bind(ctx.txn()?, "dxf", "model.dxf", &p.handle, id)?;
        Ok(json!({}))
    }
    env.commands
        .register(
            CommandSpec::new(
                "core.test.bind_source",
                "test-bind-label",
                "test-bind-description",
                run,
            )
            .with_permissions([Permission::Edit]),
        )
        .unwrap();
    env.run(
        "core.test.bind_source",
        json!({"id": id.to_string(), "handle": format!("{handle:X}")}),
    );
}

#[test]
fn under_keep_sources_only_the_first_reference_takes_the_array_handle() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let a = array(
        &mut env,
        space,
        block,
        json!({"array": {"kind": "rectangular", "rows": 2, "columns": 2,
                         "row_spacing": 3.0, "column_spacing": 3.0}}),
    );
    bind(&mut env, a, 0x500);
    let exported = export_drawing_limited(
        &env.snapshot(),
        space,
        &HandlePolicy::KeepSources {
            document: "model.dxf".to_owned(),
            floor: 0,
        },
        &Limits::DEFAULT,
    )
    .unwrap();
    let handles: Vec<u64> = exported
        .drawing
        .entities
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::Insert(_)))
        .map(|e| e.head.handle.unwrap().0)
        .collect();
    assert_eq!(handles.len(), 4);
    assert_eq!(
        handles[0], 0x500,
        "the first reference keeps the bound handle"
    );
    assert!(handles[1..].iter().all(|h| *h != 0x500));
    let mut unique = handles.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), handles.len(), "no handle is written twice");
    assert!(exported.handles.contains(&(a, arc_io_dxf::Handle(0x500))));
}

#[test]
fn the_export_budget_fails_with_a_limit_error_before_references_are_built() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    array(
        &mut env,
        space,
        block,
        json!({"array": {"kind": "rectangular", "rows": 2, "columns": 3, "column_spacing": 1.0}}),
    );
    let snapshot = env.snapshot();
    let limits = |n| Limits {
        record_count: n,
        ..Limits::DEFAULT
    };
    let error =
        export_drawing_limited(&snapshot, space, &HandlePolicy::Assign, &limits(5)).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(export_drawing_limited(&snapshot, space, &HandlePolicy::Assign, &limits(6)).is_ok());
}

#[test]
fn an_array_of_a_million_items_is_refused_when_it_is_created() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let error = env
        .invoke(
            "core.draft.array.create",
            json!({"owner": space.to_string(), "block": block.to_string(),
                   "array": {"kind": "rectangular", "rows": 1000, "columns": 1000}}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.array.too_many_items");
}

#[test]
fn an_array_whose_placements_fail_is_unmapped_and_the_rest_is_written() {
    let mut env = Env::new();
    let (space, block) = space_with_block(&mut env);
    let path = env.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": segment_json(0.0, 0.0, 10.0, 0.0)}),
    );
    let a = array(
        &mut env,
        space,
        block,
        json!({"array": {"kind": "path", "path": path.to_string(), "count": 3}}),
    );
    env.run(
        "core.draft.entity.delete",
        json!({"objects": [path.to_string()]}),
    );
    assert!(expansion(&env.snapshot(), a, &Tolerance::DEFAULT).is_err());
    let exported = export_drawing_limited(
        &env.snapshot(),
        space,
        &HandlePolicy::Assign,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(
        exported
            .drawing
            .entities
            .iter()
            .all(|e| !matches!(e.kind, EntityKind::Insert(_))),
        "no reference is written for the broken array"
    );
    assert!(
        exported
            .report
            .items
            .iter()
            .any(|i| i.reason == "draft.dxf.export_unmapped"),
        "{:?}",
        exported.report.items
    );
    assert!(exported.handles.iter().all(|(id, _)| *id != a));
}
