//! Limits, cancellation and hostile input: every limit stops the read with `io.limit_exceeded`
//! naming it, cancellation stops reads and writes mid-stream, and mutated bytes never panic.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use arc_base::{Cancel, Limits};
use arc_io_dxf::model::{Line, Point};
use arc_io_dxf::{
    Drawing, Entity, EntityKind, Progress, ReadOptions, Version, WriteOptions, read, write,
};
use common::{golden, to_binary};
use proptest::prelude::*;

fn limit_name(bytes: &[u8], limits: Limits) -> String {
    let e = read(bytes, &mut ReadOptions::with_limits(limits)).unwrap_err();
    assert_eq!(e.code(), "io.limit_exceeded");
    match e.arguments().get("limit") {
        Some(arc_base::error::Value::Text(name)) => name.clone(),
        other => panic!("no limit argument: {other:?}"),
    }
}

#[test]
fn every_limit_is_enforced_and_named() {
    let r2000 = golden("r2000.dxf");
    let r12 = golden("r12.dxf");
    let cases = [
        (
            &r2000,
            Limits {
                input_bytes: 100,
                ..Limits::STRICT
            },
            "input_bytes",
        ),
        (
            &r2000,
            Limits {
                record_count: 3,
                ..Limits::STRICT
            },
            "record_count",
        ),
        (
            &r2000,
            Limits {
                string_bytes: 5,
                ..Limits::STRICT
            },
            "string_bytes",
        ),
        (
            &r2000,
            Limits {
                array_length: 4,
                ..Limits::STRICT
            },
            "array_length",
        ),
        // Block entities sit at depth 3.
        (
            &r12,
            Limits {
                nesting_depth: 2,
                ..Limits::STRICT
            },
            "nesting_depth",
        ),
    ];
    for (bytes, limits, name) in cases {
        assert_eq!(limit_name(bytes, limits), name);
        for short in [true, false] {
            assert_eq!(
                limit_name(&to_binary(bytes, short), limits),
                name,
                "binary {name}"
            );
        }
    }
}

#[test]
fn nested_application_groups_count_toward_the_nesting_limit() {
    let mut text = String::from("0\nSECTION\n2\nENTITIES\n0\nLINE\n");
    for _ in 0..40 {
        text.push_str("102\n{X\n");
    }
    text.push_str("0\nENDSEC\n0\nEOF\n");
    assert_eq!(limit_name(text.as_bytes(), Limits::STRICT), "nesting_depth");
}

#[test]
fn declared_counts_are_checked_before_reading_elements() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nHATCH\n91\n2000000000\n0\nENDSEC\n0\nEOF\n";
    assert_eq!(
        limit_name(
            text.as_bytes(),
            Limits {
                array_length: 1000,
                ..Limits::STRICT
            }
        ),
        "array_length"
    );
}

#[test]
fn an_unterminated_binary_string_stops_at_the_string_limit() {
    let mut data = b"AutoCAD Binary DXF\r\n\x1a\0".to_vec();
    data.extend_from_slice(&[0, 0]);
    data.extend(std::iter::repeat_n(b'A', 4096));
    assert_eq!(
        limit_name(
            &data,
            Limits {
                string_bytes: 100,
                ..Limits::STRICT
            }
        ),
        "string_bytes"
    );
}

fn many_lines(n: usize) -> Vec<u8> {
    let mut text = String::from("0\nSECTION\n2\nENTITIES\n");
    for i in 0..n {
        text.push_str(&format!(
            "0\nLINE\n8\n0\n10\n{i}\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n"
        ));
    }
    text.push_str("0\nENDSEC\n0\nEOF\n");
    text.into_bytes()
}

#[test]
fn cancellation_stops_a_read_mid_stream() {
    let data = many_lines(5000);
    let cancel = Cancel::new();
    let mut seen = Vec::new();
    let token = cancel.clone();
    let mut on_progress = |p: Progress| {
        seen.push(p);
        token.cancel();
    };
    let mut options = ReadOptions {
        cancel,
        progress: Some(&mut on_progress),
        ..ReadOptions::default()
    };
    let e = read(&data, &mut options).unwrap_err();
    assert_eq!(e.code(), "base.cancelled");
    assert_eq!(seen.len(), 1);
    assert!(seen[0].done < seen[0].total);
}

#[test]
fn progress_reaches_the_end_of_a_read() {
    let data = many_lines(3000);
    let mut last = None;
    let mut on_progress = |p: Progress| last = Some(p);
    let mut options = ReadOptions {
        progress: Some(&mut on_progress),
        ..ReadOptions::default()
    };
    read(&data, &mut options).unwrap();
    let last = last.unwrap();
    assert_eq!(last.done, last.total);
}

#[test]
fn cancellation_stops_a_write_mid_stream() {
    let mut d = Drawing::default();
    for i in 0..5000 {
        let kind = if i % 2 == 0 {
            EntityKind::Line(Line::default())
        } else {
            EntityKind::Point(Point::default())
        };
        d.entities.push(Entity::new(kind));
    }
    let cancel = Cancel::new();
    let token = cancel.clone();
    let mut calls = 0;
    let mut on_progress = |p: Progress| {
        calls += 1;
        assert!(p.done < p.total);
        token.cancel();
    };
    let mut options = WriteOptions {
        version: Version::R2018,
        cancel,
        progress: Some(&mut on_progress),
        ..WriteOptions::default()
    };
    let e = write(&d, &mut options).unwrap_err();
    assert_eq!(e.code(), "base.cancelled");
    assert_eq!(calls, 1);
}

#[test]
fn every_prefix_of_every_golden_file_fails_cleanly_or_reads() {
    for name in ["r12.dxf", "r2000.dxf", "r2018.dxf"] {
        let ascii = golden(name);
        for bytes in [ascii.clone(), to_binary(&ascii, false)] {
            for end in 0..bytes.len() {
                let _ = read(&bytes[..end], &mut ReadOptions::default());
            }
        }
    }
}

/// Small enough for mutated inputs to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 4096,
    record_count: 40,
    nesting_depth: 4,
    string_bytes: 64,
    array_length: 64,
    ..Limits::STRICT
};

fn exercise(bytes: &[u8]) {
    for limits in [TIGHT, Limits::STRICT] {
        if let Ok(out) = read(bytes, &mut ReadOptions::with_limits(limits)) {
            for binary in [false, true] {
                let options = &mut WriteOptions {
                    binary,
                    ..WriteOptions::default()
                };
                if let Ok(written) = write(&out.drawing, options) {
                    let _ = read(
                        &written.bytes,
                        &mut ReadOptions::with_limits(Limits::STRICT),
                    );
                }
            }
        }
    }
}

proptest! {
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        exercise(&bytes);
        let mut binary = b"AutoCAD Binary DXF\r\n\x1a\0".to_vec();
        binary.extend_from_slice(&bytes);
        exercise(&binary);
    }

    #[test]
    fn mutated_golden_files_never_panic(
        file in 0usize..4,
        binary in any::<bool>(),
        edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0u8..3), 1..8),
    ) {
        let name = ["r12.dxf", "r2000.dxf", "r2007.dxf", "r2018.dxf"][file];
        let ascii = golden(name);
        let mut bytes = if binary { to_binary(&ascii, file == 0) } else { ascii };
        for (at, byte, op) in edits {
            let i = at.index(bytes.len().max(1));
            match op {
                0 if i < bytes.len() => bytes[i] = byte,
                1 => bytes.insert(i.min(bytes.len()), byte),
                _ => bytes.truncate(i),
            }
        }
        exercise(&bytes);
    }
}
