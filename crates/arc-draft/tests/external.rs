//! External drawing references (CORE-DRAFT-025): display of linked drawings through loaded
//! links, clips, unresolved and self-referencing links, read-only use, and binding into a block.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result};
use arc_doc::link::{self, LinkSpec, LinkState, Links, LoadMode, Opened};
use arc_doc::{Snapshot, Txn};
use arc_draft::display::{DisplayList, DisplayOptions, Lists, Shape, space_list_with_links};
use arc_draft::external_commands::bind_command;
use arc_geom::{Aabb2, Circle2, Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::json;
use support::{Fixture, curve_json};

const HOST: u128 = 1;
const LINKED: u128 = 2;
const LINK: u128 = 10;
const LINK_BACK: u128 = 11;

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn segment(a: (f64, f64), b: (f64, f64)) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)).unwrap())
}

fn circle(center: (f64, f64), radius: f64) -> Curve2 {
    Curve2::Circle(Circle2::new(Point2::new(center.0, center.1), radius).unwrap())
}

/// A closed rectangle from `lo` to `hi`, in the coordinates of the linked drawing.
fn rectangle(lo: (f64, f64), hi: (f64, f64)) -> serde_json::Value {
    let vertices = vec![
        PolylineVertex::new(Point2::new(lo.0, lo.1), 0.0),
        PolylineVertex::new(Point2::new(hi.0, lo.1), 0.0),
        PolylineVertex::new(Point2::new(hi.0, hi.1), 0.0),
        PolylineVertex::new(Point2::new(lo.0, hi.1), 0.0),
    ];
    curve_json(&Curve2::Polyline(Polyline2::new(vertices, true).unwrap()))
}

/// The linked drawing: a line from (0, 0) to (10, 0) and a circle of radius 2 at (5, 5).
fn drawing() -> Fixture {
    let mut f = Fixture::drafting();
    let space = f.space("Model");
    f.entity(space, curve_json(&segment((0.0, 0.0), (10.0, 0.0))));
    f.entity(space, curve_json(&circle((5.0, 5.0), 2.0)));
    f
}

/// A host document with a link object `link` to `document` at `path`, and a space.
fn host(document: ObjectId, path: &'static str) -> (Fixture, ObjectId) {
    let mut f = Fixture::drafting_with_links();
    let space = f.space("Host");
    let link = id(LINK);
    let spec = LinkSpec {
        document,
        path,
        hub_resource: None,
        mode: LoadMode::Full,
    };
    f.document
        .submit(move |txn: &mut Txn| link::create(txn, link, &spec))
        .wait()
        .unwrap();
    (f, space)
}

/// A resolver that opens `snapshot` as `document` for every path.
fn opening(document: ObjectId, snapshot: Snapshot) -> impl Fn(&str, &str) -> Result<Opened> {
    move |_from: &str, path: &str| {
        Ok(Opened {
            location: path.to_owned(),
            document,
            content: [7; 32],
            snapshot: snapshot.clone(),
        })
    }
}

/// Loads the links of the host `f` (root identifier `HOST`) through `resolver`.
fn load(f: &Fixture, resolver: &dyn link::LinkResolver) -> Links {
    link::load(
        &f.document.snapshot(),
        id(HOST),
        "host.arc",
        resolver,
        &Limits::DEFAULT,
    )
}

fn place(f: &mut Fixture, space: ObjectId, position: [f64; 2]) -> ObjectId {
    f.create(
        "core.draft.external_ref.create",
        json!({"owner": space.to_string(), "link": id(LINK).to_string(), "position": position}),
    )
}

fn list(f: &Fixture, space: ObjectId, links: Option<&Links>) -> DisplayList {
    space_list_with_links(
        &f.document.snapshot(),
        Lists::Direct,
        links,
        space,
        &DisplayOptions::default(),
    )
    .unwrap()
}

/// The bounding boxes of the drawn parts of a primitive.
fn boxes(shape: &Shape) -> Vec<Aabb2> {
    match shape {
        Shape::Stroke { curve, drawn: None } => vec![curve.bounding_box().unwrap()],
        Shape::Stroke {
            drawn: Some(pieces),
            ..
        } => pieces
            .iter()
            .filter_map(|piece| match piece {
                arc_draft::clip::Piece::Curve(c) => c.bounding_box(),
                arc_draft::clip::Piece::Point(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn linked_snapshot() -> Snapshot {
    drawing().document.snapshot()
}

#[test]
fn the_linked_drawing_is_drawn_transformed_with_source_identifiers_carrying_the_link() {
    let linked = linked_snapshot();
    let (mut f, space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut f, space, [100.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let out = list(&f, space, Some(&links));
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert_eq!(out.primitives.len(), 2);

    let line = &out.primitives[0];
    assert_eq!(line.source, placed);
    assert_eq!(line.path[0], id(LINK), "the link object comes first");
    assert_eq!(line.path.len(), 2, "then the object in the linked drawing");
    let bounds = boxes(&line.shape);
    assert_eq!(bounds.len(), 1);
    assert!((bounds[0].min().x - 100.0).abs() < 1e-9);
    assert!((bounds[0].max().x - 110.0).abs() < 1e-9);

    let circle = &out.primitives[1];
    let bounds = boxes(&circle.shape);
    assert!((bounds[0].min().x - 103.0).abs() < 1e-9, "{:?}", bounds[0]);
    assert!((bounds[0].max().x - 107.0).abs() < 1e-9);
    assert_eq!(circle.path[0], id(LINK));
    assert_ne!(
        circle.path[1], line.path[1],
        "each linked entity has its own identifier"
    );
}

#[test]
fn a_clip_boundary_in_linked_coordinates_keeps_only_the_inside_parts() {
    let (mut f, space) = host(id(LINKED), "linked.arc");
    let placed = f.create(
        "core.draft.external_ref.create",
        json!({
            "owner": space.to_string(),
            "link": id(LINK).to_string(),
            "position": [100.0, 0.0],
            "clip": rectangle((-1.0, -1.0), (6.0, 6.0)),
        }),
    );
    let linked = linked_snapshot();
    let links = load(&f, &opening(id(LINKED), linked));
    let out = list(&f, space, Some(&links));
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    // The line is cut at x = 6; the circle (x from 3 to 7) is cut at x = 6 too.
    let mut lines = 0;
    for p in &out.primitives {
        assert_eq!(p.source, placed);
        for b in boxes(&p.shape) {
            assert!(b.max().x <= 106.0 + 1e-6, "{b:?} lies outside the clip");
            assert!(b.min().x >= 100.0 - 1e-6);
        }
        if boxes(&p.shape)
            .iter()
            .any(|b| (b.max().x - 106.0).abs() < 1e-6)
        {
            lines += 1;
        }
    }
    assert!(
        lines >= 2,
        "the cut line and the cut circle both reach the clip edge"
    );
    // Changing the clip with set_clip removes it again.
    f.run(
        "core.draft.external_ref.set_clip",
        json!({"objects": [placed.to_string()], "clip": null}),
    )
    .unwrap();
    let links = load(&f, &opening(id(LINKED), linked_snapshot()));
    let unclipped = list(&f, space, Some(&links));
    assert!(boxes(&unclipped.primitives[0].shape)[0].max().x > 109.0);
}

#[test]
fn a_missing_or_unresolved_link_draws_nothing_and_reports_a_problem() {
    let (mut f, space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut f, space, [0.0, 0.0]);
    let failing = |_: &str, _: &str| -> Result<Opened> {
        Err(arc_base::Error::new(arc_doc::error::codes::LINK_UNRESOLVED))
    };
    let links = load(&f, &failing);
    assert!(matches!(
        links.state(id(LINK)),
        Some(LinkState::Unresolved(_))
    ));
    let out = list(&f, space, Some(&links));
    assert!(out.primitives.is_empty());
    assert_eq!(out.problems.len(), 1);
    assert_eq!(out.problems[0].reference, placed);
    assert_eq!(out.problems[0].code, "doc.link.unresolved");
    // Nothing is deleted.
    let snapshot = f.document.snapshot();
    assert!(snapshot.resolve(placed).is_ok());
    assert!(snapshot.resolve(id(LINK)).is_ok());

    // Without loaded links the reference draws nothing either.
    let without = list(&f, space, None);
    assert!(without.primitives.is_empty());
    assert_eq!(without.problems[0].code, "draft.external_ref.no_links");
}

#[test]
fn a_drawing_that_references_itself_through_links_is_cut_at_the_limit_and_reported() {
    // The linked drawing D holds a placed reference back to the host H.
    let mut d = Fixture::drafting_with_links();
    let d_space = d.space("Model");
    d.entity(d_space, curve_json(&segment((0.0, 0.0), (10.0, 0.0))));
    let back = id(LINK_BACK);
    let spec = LinkSpec {
        document: id(HOST),
        path: "host.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    d.document
        .submit(move |txn: &mut Txn| link::create(txn, back, &spec))
        .wait()
        .unwrap();
    d.create(
        "core.draft.external_ref.create",
        json!({"owner": d_space.to_string(), "link": back.to_string(), "position": [0.0, 0.0]}),
    );
    let (mut h, h_space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut h, h_space, [0.0, 0.0]);

    let resolver = {
        let d_snapshot = d.document.snapshot();
        let h_snapshot = h.document.snapshot();
        move |_: &str, path: &str| -> Result<Opened> {
            match path {
                "linked.arc" => Ok(Opened {
                    location: path.to_owned(),
                    document: id(LINKED),
                    content: [7; 32],
                    snapshot: d_snapshot.clone(),
                }),
                _ => Ok(Opened {
                    location: path.to_owned(),
                    document: id(HOST),
                    content: [8; 32],
                    snapshot: h_snapshot.clone(),
                }),
            }
        }
    };
    let links = load(&h, &resolver);
    let out = list(&h, h_space, Some(&links));
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    assert_eq!(out.problems[0].code, "draft.block.nesting_too_deep");
    assert_eq!(out.problems[0].reference, placed);
    assert!(
        !out.primitives.is_empty(),
        "the levels above the limit still draw"
    );
    assert!(out.primitives.iter().all(|p| p.source == placed));
}

#[test]
fn bind_produces_a_block_that_displays_like_the_reference_and_one_undo_restores_it() {
    let linked = linked_snapshot();
    let (mut f, space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut f, space, [100.0, 20.0]);
    let link_resolver = opening(id(LINKED), linked.clone());
    let links = load(&f, &link_resolver);
    let before = list(&f, space, Some(&links));
    assert_eq!(before.primitives.len(), 2);

    f.commands.register(bind_command(links.clone())).unwrap();
    let inserted: ObjectId = f.create(
        "core.draft.external_ref.bind",
        json!({"reference": placed.to_string()}),
    );

    let snapshot = f.document.snapshot();
    assert!(
        snapshot.resolve(placed).is_err(),
        "the placed reference is replaced"
    );
    assert!(
        snapshot.resolve(id(LINK)).is_err(),
        "the unused link is deleted"
    );
    let after = list(&f, space, None);
    assert_eq!(after.primitives.len(), before.primitives.len());
    for (a, b) in before.primitives.iter().zip(&after.primitives) {
        assert_eq!(a.shape, b.shape, "same geometry after the bind");
        assert_eq!(
            a.color.as_ref().map(|c| c.value.clone()),
            b.color.as_ref().map(|c| c.value.clone())
        );
        assert_eq!(b.source, inserted);
    }

    // One undo restores the reference and the link, and removes the block.
    f.document.undo().wait().unwrap();
    let snapshot = f.document.snapshot();
    assert!(snapshot.resolve(placed).is_ok());
    assert!(snapshot.resolve(id(LINK)).is_ok());
    assert!(snapshot.resolve(inserted).is_err());
    assert_eq!(snapshot.iter_type("core.draft.block").unwrap().count(), 0);
    let restored = list(&f, space, Some(&load(&f, &link_resolver)));
    assert_eq!(restored.primitives.len(), 2);
}

#[test]
fn commands_on_the_host_leave_the_linked_snapshot_unchanged() {
    let linked = linked_snapshot();
    let (mut f, space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let revision = links.snapshot().unwrap().revision();
    let linked_before = links
        .linked(id(LINK))
        .unwrap()
        .snapshot()
        .unwrap()
        .revision();

    f.run(
        "core.draft.entity.modify",
        json!({"objects": [placed.to_string()], "field": "position", "value": [50.0, 50.0]}),
    )
    .unwrap();
    let moved = list(
        &f,
        space,
        Some(&load(&f, &opening(id(LINKED), linked.clone()))),
    );
    assert_eq!(moved.primitives.len(), 2);
    assert_eq!(links.snapshot().unwrap().revision(), revision);
    assert_eq!(
        links
            .linked(id(LINK))
            .unwrap()
            .snapshot()
            .unwrap()
            .revision(),
        linked_before
    );
    let bounds = boxes(&moved.primitives[0].shape);
    assert!(
        (bounds[0].min().x - 50.0).abs() < 1e-9,
        "the reference moved, the drawing did not"
    );
}

#[test]
fn creating_references_checks_the_link_clip_and_owner() {
    let (mut f, space) = host(id(LINKED), "linked.arc");
    // A closed polyline is required for a clip.
    let open = json!({"kind": "segment", "start": [0.0, 0.0], "end": [1.0, 0.0]});
    let err = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": space.to_string(), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": open}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "draft.external_ref.clip_invalid");
    // The link must be a link object.
    let err = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": space.to_string(), "link": space.to_string(), "position": [0.0, 0.0]}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "draft.object.wrong_type");
}

#[test]
fn bind_copies_a_layer_whose_name_is_taken_under_a_prefixed_name() {
    // The linked drawing has a red layer "Walls" holding the line; the host has its own "Walls".
    let mut d = Fixture::drafting();
    let d_space = d.space("Model");
    let red = d.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [1.0, 0.0, 0.0, 1.0]}),
    );
    d.create(
        "core.draft.entity.create",
        json!({
            "owner": d_space.to_string(),
            "geometry": curve_json(&segment((0.0, 0.0), (10.0, 0.0))),
            "layer": red.to_string(),
        }),
    );
    let linked = d.document.snapshot();
    let (mut f, space) = host(id(LINKED), "linked.arc");
    f.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [0.0, 0.0, 1.0, 1.0]}),
    );
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked));
    let before = list(&f, space, Some(&links));
    assert_eq!(before.primitives.len(), 1);

    f.commands.register(bind_command(links.clone())).unwrap();
    f.create(
        "core.draft.external_ref.bind",
        json!({"reference": placed.to_string()}),
    );

    let snapshot = f.document.snapshot();
    let names: Vec<String> = snapshot
        .iter_type("core.draft.layer")
        .unwrap()
        .filter_map(|(_, o)| match o.values.first() {
            Some(Some(arc_doc::Value::String(name))) => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"Walls".to_owned()), "{names:?}");
    assert!(names.contains(&"linked-Walls".to_owned()), "{names:?}");

    let after = list(&f, space, None);
    assert_eq!(after.primitives.len(), 1);
    assert_eq!(before.primitives[0].shape, after.primitives[0].shape);
    assert_eq!(
        before.primitives[0].color.as_ref().map(|c| c.value.clone()),
        after.primitives[0].color.as_ref().map(|c| c.value.clone()),
        "the copied layer keeps the red of the linked drawing"
    );
    // The copied entity is on the copy of the layer, not the host's "Walls".
    let copy = after.primitives[0].layers[0];
    let name = match f.values(copy).first() {
        Some(Some(arc_doc::Value::String(name))) => name.clone(),
        _ => String::new(),
    };
    assert_eq!(name, "linked-Walls");
}

/// A drawing D with a line and `refs` placed references back to the host, and a host H with one
/// placed reference to D. Returns H, its space, its placed reference, and a loaded view of H.
fn eight_back_references(refs: usize) -> (Fixture, ObjectId, ObjectId, Links) {
    let mut d = Fixture::drafting_with_links();
    let d_space = d.space("Model");
    d.entity(d_space, curve_json(&segment((0.0, 0.0), (10.0, 0.0))));
    let back = id(LINK_BACK);
    let spec = LinkSpec {
        document: id(HOST),
        path: "host.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    d.document
        .submit(move |txn: &mut Txn| link::create(txn, back, &spec))
        .wait()
        .unwrap();
    for k in 0..refs {
        d.create(
            "core.draft.external_ref.create",
            json!({"owner": d_space.to_string(), "link": back.to_string(), "position": [k as f64, 0.0]}),
        );
    }
    let (mut h, h_space) = host(id(LINKED), "linked.arc");
    let placed = place(&mut h, h_space, [0.0, 0.0]);
    let (d_snapshot, h_snapshot) = (d.document.snapshot(), h.document.snapshot());
    let resolver = move |_: &str, path: &str| -> Result<Opened> {
        let (document, snapshot) = match path {
            "linked.arc" => (id(LINKED), d_snapshot.clone()),
            _ => (id(HOST), h_snapshot.clone()),
        };
        Ok(Opened {
            location: path.to_owned(),
            document,
            content: [7; 32],
            snapshot,
        })
    };
    let links = load(&h, &resolver);
    (h, h_space, placed, links)
}

#[test]
fn eight_self_references_stay_within_the_nesting_limit_and_the_budget() {
    let (h, h_space, placed, links) = eight_back_references(8);
    // With the default budget the memo keeps the build to one expansion per document, space and
    // depth; the bound that cuts first is the nesting limit or the size of the list.
    let out = list(&h, h_space, Some(&links));
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    assert_eq!(out.problems[0].reference, placed);
    let code = out.problems[0].code;
    assert!(
        matches!(
            code,
            "draft.block.nesting_too_deep"
                | "draft.display.too_large"
                | "draft.external_ref.budget_exceeded"
        ),
        "{code}"
    );
    assert!(out.primitives.len() <= DisplayOptions::default().max_primitives);
}

#[test]
fn an_exhausted_budget_stops_the_expansions_and_reports_one_problem_per_reference() {
    let (h, h_space, placed, links) = eight_back_references(8);
    let out = arc_draft::display::space_list_with_budget(
        &h.document.snapshot(),
        Lists::Direct,
        Some(&links),
        h_space,
        &DisplayOptions::default(),
        3,
    )
    .unwrap();
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    assert_eq!(out.problems[0].reference, placed);
    assert_eq!(out.problems[0].code, "draft.external_ref.budget_exceeded");
}
