//! Snapping feedback (CORE-UI-030): every kind has its own marker shape and a message that exists
//! in both catalogs, and painting without a snap draws nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use arc_ui::egui;
use arc_ui::snap::{self, Snap, SnapKind};
use arc_ui::text::messages;

struct Names;

impl arc_ui::Localize for Names {
    fn message(&self, id: &str, _args: &[(&str, arc_base::error::Value)]) -> Option<String> {
        Some(id.trim_start_matches("ui-snap-").to_owned())
    }
}

fn painted(snap: Option<&Snap>, tracking: &[[egui::Pos2; 2]]) -> usize {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(400.0, 300.0),
        )),
        ..Default::default()
    };
    let mut full = ctx.run_ui(input, |ui| snap::paint(ui, &Names, snap, tracking));
    full.textures_delta.clear();
    full.shapes.len()
}

#[test]
fn every_kind_has_a_distinct_marker_shape() {
    let shapes: Vec<_> = SnapKind::ALL.iter().map(|k| k.marker()).collect();
    for (i, a) in shapes.iter().enumerate() {
        for b in shapes.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
    assert_eq!(shapes.len(), 9);
}

#[test]
fn every_kind_names_itself_in_both_catalogs() {
    let en = include_str!("../../../locales/en-US/arc-ui.ftl");
    let tr = include_str!("../../../locales/tr-TR/arc-ui.ftl");
    for kind in SnapKind::ALL {
        let id = kind.message().message_id;
        assert!(en.contains(&format!("\n{id} = ")), "{id} in en-US");
        assert!(tr.contains(&format!("\n{id} = ")), "{id} in tr-TR");
    }
    assert_eq!(SnapKind::Endpoint.message(), messages::SNAP_ENDPOINT);
}

#[test]
fn painting_without_a_snap_draws_nothing_and_with_one_draws_the_marker() {
    let tracking = [[egui::pos2(10.0, 10.0), egui::pos2(90.0, 10.0)]];
    assert_eq!(
        painted(None, &tracking),
        0,
        "no snap, no tracking lines either"
    );
    let snap = Snap {
        kind: SnapKind::Midpoint,
        position: egui::pos2(50.0, 50.0),
    };
    assert!(painted(Some(&snap), &tracking) > 0);
    assert!(painted(Some(&snap), &[]) > 0);
}
