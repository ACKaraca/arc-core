//! A drawing whose highest handle leaves too little room for the handles the writer assigns is
//! refused, and the handle seed never wraps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use arc_io_dxf::Group;
use arc_io_dxf::model::{HeaderVariable, Line};
use arc_io_dxf::{Drawing, Entity, EntityKind, Handle, WriteOptions, write};

fn line(handle: Option<u64>) -> Entity {
    let mut e = Entity::new(EntityKind::Line(Line {
        start: [0.0; 3],
        end: [1.0, 0.0, 0.0],
    }));
    e.head.handle = handle.map(Handle);
    e
}

#[test]
fn a_highest_handle_at_the_top_of_the_space_is_refused() {
    let drawing = Drawing {
        entities: vec![line(Some(u64::MAX)), line(None)],
        ..Drawing::default()
    };
    assert!(write(&drawing, &mut WriteOptions::default()).is_err());
}

#[test]
fn a_highest_handle_with_room_above_it_is_written_and_the_seed_is_above_it() {
    let drawing = Drawing {
        entities: vec![line(Some(0x10)), line(None)],
        header: vec![HeaderVariable {
            name: "$HANDSEED".to_owned(),
            values: vec![Group::text(5, "1")],
        }],
        ..Drawing::default()
    };
    let out = write(&drawing, &mut WriteOptions::default()).unwrap();
    let text = String::from_utf8(out.bytes).unwrap();
    let seed = text
        .lines()
        .skip_while(|l| *l != "$HANDSEED")
        .nth(2)
        .unwrap();
    assert_eq!(u64::from_str_radix(seed.trim(), 16).unwrap(), 0x12);
}
