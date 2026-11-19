//! Hostile input: arbitrary and mutated bytes return `Ok` or an error and never panic; a declared
//! count the file cannot hold is refused at once; every limit is enforced and named.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "test data"
)]

use std::time::{Duration, Instant};

use arc_base::error::Value;
use arc_base::math::Point3;
use arc_base::{Cancel, Error, Limits};
use arc_io_mesh::{Encoding, MeshPart, MeshScene, ReadOptions, WriteOptions, obj, ply, stl};
use arc_mesh::{AttributeKey, Channel, TriMesh, TriMeshBuilder};
use proptest::prelude::*;

const CORNERS: [[f64; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 1.0, 1.0],
];

const FACES: [[u32; 3]; 12] = [
    [0, 2, 1],
    [0, 3, 2],
    [4, 5, 6],
    [4, 6, 7],
    [0, 1, 5],
    [0, 5, 4],
    [1, 2, 6],
    [1, 6, 5],
    [2, 3, 7],
    [2, 7, 6],
    [3, 0, 4],
    [3, 4, 7],
];

/// The cube with normals and texture coordinates, so that every writer has its full path.
fn cube() -> TriMesh {
    let positions = CORNERS
        .iter()
        .map(|p| Point3::new(p[0], p[1], p[2]))
        .collect();
    TriMeshBuilder::new(positions, FACES.to_vec())
        .channel(
            AttributeKey::Normal,
            Channel::Vec3(vec![[0.0, 0.0, 1.0]; 8]),
        )
        .channel(AttributeKey::Uv(0), Channel::Vec2(vec![[0.25, 0.75]; 8]))
        .build(&Limits::DEFAULT)
        .unwrap()
}

fn cube_scene() -> MeshScene {
    MeshScene {
        meshes: vec![MeshPart {
            name: "cube".to_owned(),
            mesh: cube(),
            material: None,
            preserved: Default::default(),
        }],
        materials: Vec::new(),
        ..MeshScene::default()
    }
}

/// Valid files of every format and encoding, for mutation.
fn valid_files() -> Vec<Vec<u8>> {
    let scene = cube_scene();
    let mut files = vec![
        obj::write(&scene, &mut WriteOptions::default())
            .unwrap()
            .bytes,
    ];
    for encoding in [Encoding::Ascii, Encoding::BinaryLittleEndian] {
        files.push(
            stl::write(&scene, &mut WriteOptions::with_encoding(encoding))
                .unwrap()
                .bytes,
        );
    }
    for encoding in [
        Encoding::Ascii,
        Encoding::BinaryLittleEndian,
        Encoding::BinaryBigEndian,
    ] {
        files.push(
            ply::write(&scene, &mut WriteOptions::with_encoding(encoding))
                .unwrap()
                .bytes,
        );
    }
    files
}

/// Each reader's outcome must be `Ok` or an error with a code of this crate family.
fn assert_clean<T>(result: arc_base::Result<T>) {
    if let Err(error) = result {
        let code = error.code();
        assert!(
            code.starts_with("io.") || code.starts_with("mesh.") || code.starts_with("base."),
            "unexpected code {code}"
        );
    }
}

fn read_everything(bytes: &[u8]) {
    assert_clean(obj::read(bytes, Some(bytes), &mut ReadOptions::default()));
    assert_clean(stl::read(bytes, &mut ReadOptions::default()));
    assert_clean(ply::read(bytes, &mut ReadOptions::default()));
}

/// Tokens that steer the readers into their statement and header paths.
const TOKENS: &[&str] = &[
    "v",
    "vt",
    "vn",
    "f",
    "o",
    "g",
    "usemtl",
    "mtllib",
    "newmtl",
    "Kd",
    "map_Kd",
    "solid",
    "endsolid",
    "facet",
    "normal",
    "outer",
    "loop",
    "vertex",
    "endloop",
    "endfacet",
    "ply",
    "format",
    "ascii",
    "binary_little_endian",
    "1.0",
    "element",
    "property",
    "list",
    "uchar",
    "uint",
    "float",
    "double",
    "end_header",
    "comment",
    "x",
    "y",
    "z",
    "vertex_indices",
    "-1",
    "0",
    "1",
    "2",
    "3",
    "99",
    "4000000000",
    "1e400",
    "nan",
    "-0",
    "1/2/3",
    "1//2",
    "#",
    "\\",
];

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_any_reader(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        read_everything(&bytes);
    }

    #[test]
    fn statement_like_text_never_panics(
        words in prop::collection::vec(
            (prop::sample::select(TOKENS), prop::bool::ANY),
            0..120,
        ),
    ) {
        let text: String = words
            .iter()
            .map(|(word, newline)| if *newline { format!("{word}\n") } else { format!("{word} ") })
            .collect();
        read_everything(text.as_bytes());
    }

    #[test]
    fn mutated_valid_files_never_panic(
        which in 0_usize..6,
        flips in prop::collection::vec((any::<usize>(), any::<u8>()), 0..12),
        cut in any::<usize>(),
    ) {
        let files = valid_files();
        let mut bytes = files[which % files.len()].clone();
        for (at, value) in flips {
            if !bytes.is_empty() {
                let index = at % bytes.len();
                bytes[index] = value;
            }
        }
        let keep = cut % (bytes.len() + 1);
        bytes.truncate(keep);
        read_everything(&bytes);
    }
}

#[test]
fn valid_files_read_back_without_loss_of_the_cube() {
    for bytes in valid_files() {
        read_everything(&bytes);
    }
    let obj_bytes = &valid_files()[0];
    let read = obj::read(obj_bytes, None, &mut ReadOptions::default()).unwrap();
    assert_eq!(read.scene.meshes[0].mesh.triangle_count(), 12);
}

fn limit_name(error: &Error) -> String {
    match error.arguments().get("limit") {
        Some(Value::Text(name)) => name.clone(),
        other => panic!("no limit argument: {other:?} for {}", error.code()),
    }
}

#[test]
fn a_declared_count_the_file_cannot_hold_is_refused_at_once() {
    let started = Instant::now();
    let huge_ascii =
        b"ply\nformat ascii 1.0\nelement vertex 4000000000\nproperty float x\nend_header\n0\n";
    let error = ply::read(huge_ascii, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert_eq!(limit_name(&error), "record_count");

    let mut binary = b"ply\nformat binary_little_endian 1.0\nelement vertex 9000000\nproperty double x\nend_header\n".to_vec();
    binary.extend_from_slice(&[0; 10]);
    let error = ply::read(&binary, &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.mesh.truncated");

    let mut big_limits = binary.clone();
    big_limits.truncate(binary.len());
    let error = ply::read(&big_limits, &mut ReadOptions::with_limits(Limits::DEFAULT)).unwrap_err();
    assert_eq!(error.code(), "io.mesh.truncated");

    let mut stl_file = vec![0_u8; 80];
    stl_file.extend_from_slice(&u32::MAX.to_le_bytes());
    stl_file.extend_from_slice(&[0; 50]);
    let error = stl::read(&stl_file, &mut ReadOptions::default()).unwrap_err();
    assert!(error.code().starts_with("io.mesh."), "{}", error.code());

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "refusal took too long"
    );
}

#[test]
fn every_limit_is_enforced_and_named() {
    let obj_bytes = obj::write(&cube_scene(), &mut WriteOptions::default())
        .unwrap()
        .bytes;
    let stl_bytes = stl::write(&cube_scene(), &mut WriteOptions::default())
        .unwrap()
        .bytes;
    let stl_binary = stl::write(
        &cube_scene(),
        &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian),
    )
    .unwrap()
    .bytes;
    let ply_bytes = ply::write(&cube_scene(), &mut WriteOptions::default())
        .unwrap()
        .bytes;
    let tiny_input = Limits {
        input_bytes: 10,
        ..Limits::STRICT
    };
    let few_records = Limits {
        record_count: 3,
        ..Limits::STRICT
    };
    let short_strings = Limits {
        string_bytes: 4,
        ..Limits::STRICT
    };
    let short_arrays = Limits {
        array_length: 2,
        ..Limits::STRICT
    };
    let cases: [(&str, &[u8], Limits, &str); 9] = [
        ("input_bytes", &obj_bytes, tiny_input, "obj"),
        ("record_count", &obj_bytes, few_records, "obj"),
        ("string_bytes", &obj_bytes, short_strings, "obj"),
        ("array_length", &obj_bytes, short_arrays, "obj"),
        ("record_count", &stl_bytes, few_records, "stl"),
        ("record_count", &stl_binary, few_records, "stl"),
        ("record_count", &ply_bytes, few_records, "ply"),
        ("string_bytes", &ply_bytes, short_strings, "ply"),
        ("array_length", &ply_bytes, short_arrays, "ply"),
    ];
    for (name, bytes, limits, format) in cases {
        let error = match format {
            "obj" => obj::read(bytes, None, &mut ReadOptions::with_limits(limits)).unwrap_err(),
            "stl" => stl::read(bytes, &mut ReadOptions::with_limits(limits)).unwrap_err(),
            _ => ply::read(bytes, &mut ReadOptions::with_limits(limits)).unwrap_err(),
        };
        assert_eq!(error.code(), "io.limit_exceeded", "{format} {name}");
        assert_eq!(limit_name(&error), name, "{format}");
    }
}

#[test]
fn the_writers_refuse_names_the_format_cannot_hold() {
    let mut scene = cube_scene();
    scene.meshes[0].name = "two\nlines".to_owned();
    let error = obj::write(&scene, &mut WriteOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.mesh.write_value_invalid");
    scene.meshes[0].name = "hash#tag".to_owned();
    let error = obj::write(&scene, &mut WriteOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.mesh.write_value_invalid");
}

#[test]
fn cancelled_reads_return_cancelled_for_every_reader() {
    let cancel = Cancel::new();
    cancel.cancel();
    // The order of `valid_files`: OBJ, two STL encodings, three PLY encodings.
    let readers = ["obj", "stl", "stl", "ply", "ply", "ply"];
    for (bytes, reader) in valid_files().iter().zip(readers) {
        let mut options = ReadOptions {
            cancel: cancel.clone(),
            ..ReadOptions::default()
        };
        let error = match reader {
            "obj" => obj::read(bytes, None, &mut options).unwrap_err(),
            "stl" => stl::read(bytes, &mut options).unwrap_err(),
            _ => ply::read(bytes, &mut options).unwrap_err(),
        };
        assert_eq!(error.code(), "base.cancelled", "{reader}");
    }
}
