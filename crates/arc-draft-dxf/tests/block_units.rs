//! The unit rule of block definitions: a block keeps its content in its own unit, and the drafting
//! model converts the content at each reference. A block inserted from a file in another unit must
//! display at the converted size, whether its record names a unit or is unitless.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::ObjectId;
use arc_draft::display::{DisplayOptions, Lists, space_list};
use arc_geom::Curve2;
use arc_io_dxf::model::{Insert, Table, TableEntry};
use arc_io_dxf::{Entity, EntityKind};
use support::{Env, block, bytes, circle, drawing};

/// A file in inches: a block `DOOR` with a circle of radius 1 at its origin, record units
/// `record_units` (the `$INSUNITS` code of the block record), inserted at `(10, 0)`, with a
/// top-level circle of radius 1 at `(0, 0)` for comparison.
fn inch_file_with_door(record_units: i64) -> Vec<u8> {
    let mut d = drawing(
        vec![
            circle([0.0; 3], 1.0),
            Entity::new(EntityKind::Insert(Insert {
                block: "DOOR".to_owned(),
                insert: [10.0, 0.0, 0.0],
                ..Insert::default()
            })),
        ],
        Some(1),
    );
    d.blocks.push(block("DOOR", vec![circle([0.0; 3], 1.0)]));
    d.tables.block_records = Some(Table {
        entries: vec![TableEntry {
            name: "DOOR".to_owned(),
            flags: i16::try_from(record_units).unwrap(),
            ..TableEntry::default()
        }],
        ..Table::default()
    });
    bytes(&d)
}

/// The circles of the display list of `space`: (center x, radius) in draw order.
fn display_circles(env: &Env, space: ObjectId) -> Vec<(f64, f64)> {
    let snapshot = env.snapshot();
    let list = space_list(&snapshot, Lists::Direct, space, &DisplayOptions::default()).unwrap();
    list.primitives
        .iter()
        .filter_map(|p| match &p.shape {
            arc_draft::display::Shape::Stroke {
                curve: Curve2::Circle(c),
                ..
            } => Some((c.center().x, c.radius())),
            _ => None,
        })
        .collect()
}

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn a_block_with_a_record_unit_displays_at_the_converted_size() {
    // The record names inches, the file is in inches and the target is millimeters.
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("door-inch.dxf", &inch_file_with_door(1));
    env.import(&path, space).unwrap();
    let circles = display_circles(&env, space);
    // The top-level circle and the block circle are both one inch, which is 25.4 millimeters;
    // the insert at 10 inches is at 254 millimeters.
    assert_eq!(circles.len(), 2, "{circles:?}");
    assert_close(circles[0].1, 25.4);
    assert_close(circles[1].1, 25.4);
    assert_close(circles[1].0, 254.0);
}

#[test]
fn a_unitless_block_takes_the_unit_of_its_file() {
    // The record names no unit: the content is in the drawing's unit, as the file's own top-level
    // circle is, so it converts like that circle.
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("door-unitless.dxf", &inch_file_with_door(0));
    env.import(&path, space).unwrap();
    let circles = display_circles(&env, space);
    assert_eq!(circles.len(), 2, "{circles:?}");
    assert_close(circles[0].1, 25.4);
    assert_close(circles[1].1, 25.4);
    assert_close(circles[1].0, 254.0);
}

#[test]
fn a_block_with_a_record_unit_of_the_target_keeps_its_size() {
    // The record names millimeters in an inch file: the content is one millimeter.
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("door-mm.dxf", &inch_file_with_door(4));
    env.import(&path, space).unwrap();
    let circles = display_circles(&env, space);
    assert_eq!(circles.len(), 2, "{circles:?}");
    assert_close(circles[0].1, 25.4);
    assert_close(circles[1].1, 1.0);
}
