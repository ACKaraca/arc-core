//! The export refuses a handle space with too little room above its start, instead of wrapping
//! handles around zero, and keeps working below that limit.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_draft_dxf::{HandlePolicy, export_drawing_with};
use support::Env;

fn keep(document: &str, floor: u64) -> HandlePolicy {
    HandlePolicy::KeepSources {
        document: document.to_owned(),
        floor,
    }
}

#[test]
fn a_floor_at_the_top_of_the_handle_space_is_refused() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let snapshot = env.snapshot();
    assert!(export_drawing_with(&snapshot, space, &keep("in.dxf", u64::MAX)).is_err());
    assert!(export_drawing_with(&snapshot, space, &keep("in.dxf", u64::MAX - 0x1000)).is_err());
}

#[test]
fn a_floor_with_room_above_it_exports_and_the_handles_stay_in_range() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    env.create(
        "core.draft.layer.create",
        serde_json::json!({"name": "walls"}),
    );
    let snapshot = env.snapshot();
    let exported = export_drawing_with(&snapshot, space, &keep("in.dxf", 0x1000)).unwrap();
    for (_, handle) in &exported.handles {
        assert!(handle.0 > 0x1000, "{handle:?}");
    }
}
