//! Fixtures written by hand in each format, and the behavior the formats promise: the cube read in
//! every format, polygons and negative indices with a material library, STL binary detection by
//! length, multi-solid STL, PLY header variants, and the loss items of each writer.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "test data"
)]

use arc_base::math::Point3;
use arc_base::{Cancel, Limits};
use arc_io_mesh::{Encoding, MeshPart, MeshScene, ReadOptions, WriteOptions, obj, ply, stl};
use arc_mesh::TriMesh;

/// The cube corners, in the order the fixtures use them.
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

/// The twelve triangles of the cube, as indices of `CORNERS`.
const FACES: [[usize; 3]; 12] = [
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

const CUBE_OBJ: &str = "# cube\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

const CUBE_STL: &str = "solid cube
  facet normal 0 0 -1
    outer loop
      vertex 0 0 0
      vertex 1 1 0
      vertex 1 0 0
    endloop
  endfacet
  facet normal 0 0 -1
    outer loop
      vertex 0 0 0
      vertex 0 1 0
      vertex 1 1 0
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 0 0 1
      vertex 1 0 1
      vertex 1 1 1
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 0 0 1
      vertex 1 1 1
      vertex 0 1 1
    endloop
  endfacet
  facet normal 0 -1 0
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 1 0 1
    endloop
  endfacet
  facet normal 0 -1 0
    outer loop
      vertex 0 0 0
      vertex 1 0 1
      vertex 0 0 1
    endloop
  endfacet
  facet normal 1 0 0
    outer loop
      vertex 1 0 0
      vertex 1 1 0
      vertex 1 1 1
    endloop
  endfacet
  facet normal 1 0 0
    outer loop
      vertex 1 0 0
      vertex 1 1 1
      vertex 1 0 1
    endloop
  endfacet
  facet normal 0 1 0
    outer loop
      vertex 1 1 0
      vertex 0 1 0
      vertex 0 1 1
    endloop
  endfacet
  facet normal 0 1 0
    outer loop
      vertex 1 1 0
      vertex 0 1 1
      vertex 1 1 1
    endloop
  endfacet
  facet normal -1 0 0
    outer loop
      vertex 0 1 0
      vertex 0 0 0
      vertex 0 0 1
    endloop
  endfacet
  facet normal -1 0 0
    outer loop
      vertex 0 1 0
      vertex 0 0 1
      vertex 0 1 1
    endloop
  endfacet
endsolid cube
";

const CUBE_PLY: &str = "ply
format ascii 1.0
comment cube
element vertex 8
property float x
property float y
property float z
element face 12
property list uchar int vertex_indices
end_header
0 0 0
1 0 0
1 1 0
0 1 0
0 0 1
1 0 1
1 1 1
0 1 1
3 0 2 1
3 0 3 2
3 4 5 6
3 4 6 7
3 0 1 5
3 0 5 4
3 1 2 6
3 1 6 5
3 2 3 7
3 2 7 6
3 3 0 4
3 3 4 7
";

/// Positions of the triangles of a mesh, as corner coordinates in order.
fn triangle_points(mesh: &TriMesh) -> Vec<[[f64; 3]; 3]> {
    mesh.triangles()
        .iter()
        .map(|t| {
            t.map(|i| {
                let p = mesh.positions()[i as usize];
                [p.x, p.y, p.z]
            })
        })
        .collect()
}

fn cube_expected() -> Vec<[[f64; 3]; 3]> {
    FACES.iter().map(|f| f.map(|i| CORNERS[i])).collect()
}

fn read_ok<T>(result: arc_base::Result<T>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("read failed: {} {:?}", error.code(), error.details()),
    }
}

fn cube_ply_binary(big: bool) -> Vec<u8> {
    let mut out = format!(
        "ply\nformat {} 1.0\nelement vertex 8\nproperty float x\nproperty float y\nproperty float z\nelement face 12\nproperty list uchar int vertex_index\nend_header\n",
        if big { "binary_big_endian" } else { "binary_little_endian" }
    )
    .into_bytes();
    for corner in CORNERS {
        for v in corner {
            let v = v as f32;
            out.extend_from_slice(&if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            });
        }
    }
    for face in FACES {
        out.push(3);
        for i in face {
            let i = i as i32;
            out.extend_from_slice(&if big {
                i.to_be_bytes()
            } else {
                i.to_le_bytes()
            });
        }
    }
    out
}

fn cube_stl_binary() -> Vec<u8> {
    let mut out = vec![0_u8; 80];
    out.extend_from_slice(&12_u32.to_le_bytes());
    for face in FACES {
        let [a, b, c] = face.map(|i| CORNERS[i].map(|v| v as f32));
        for _ in 0..3 {
            out.extend_from_slice(&0_f32.to_le_bytes());
        }
        for corner in [a, b, c] {
            for v in corner {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&0_u16.to_le_bytes());
    }
    out
}

#[test]
fn the_cube_reads_identically_in_every_format() {
    let expected = cube_expected();
    let obj = read_ok(obj::read(
        CUBE_OBJ.as_bytes(),
        None,
        &mut ReadOptions::default(),
    ));
    let stl_text = read_ok(stl::read(CUBE_STL.as_bytes(), &mut ReadOptions::default()));
    let stl_bin = read_ok(stl::read(&cube_stl_binary(), &mut ReadOptions::default()));
    let ply_text = read_ok(ply::read(CUBE_PLY.as_bytes(), &mut ReadOptions::default()));
    let ply_le = read_ok(ply::read(
        &cube_ply_binary(false),
        &mut ReadOptions::default(),
    ));
    let ply_be = read_ok(ply::read(
        &cube_ply_binary(true),
        &mut ReadOptions::default(),
    ));
    for scene in [
        &obj.scene,
        &stl_text.scene,
        &stl_bin.scene,
        &ply_text.scene,
        &ply_le.scene,
        &ply_be.scene,
    ] {
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.meshes[0].mesh.vertex_count(), 8, "welded corners");
        assert_eq!(triangle_points(&scene.meshes[0].mesh), expected);
    }
    assert!(!stl_text.report.has_loss() || stl_text.report.summary.dropped > 0);
    assert_eq!(ply_text.report.summary.dropped, 0);
}

#[test]
fn obj_polygons_negative_indices_and_material_library() {
    let obj_text = "mtllib scene.mtl\n\
o first\n\
v 0 0 0\nv 2 0 0\nv 2 2 0\nv 0 2 0\nv 1 3 0\n\
vt 0 0\nvt 1 0\nvt 1 1\n\
vn 0 0 1\n\
usemtl red\n\
f 1 2 3 4\n\
f -1 -2 -3 -4 -5\n\
g second\n\
usemtl blue\n\
f 1/1/1 2/2/1 3/3/1\n\
l 1 2\n\
usemtl red\n\
f 1 3 4 # comment\n";
    let mtl = "# library\n\
newmtl red\nKd 1 0 0\nKa 0.1 0.1 0.1\nKs 0 0 0\nd 0.5\nmap_Kd -s 1 1 1 textures/red diffuse.png\n\
newmtl blue\nKd 0 0 1\nTr 0.25\nillum 2\n";
    let read = read_ok(obj::read(
        obj_text.as_bytes(),
        Some(mtl.as_bytes()),
        &mut ReadOptions::default(),
    ));
    let names: Vec<(&str, Option<&str>, usize)> = read
        .scene
        .meshes
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.material.as_deref(),
                p.mesh.triangle_count(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("first", Some("red"), 2 + 3),
            ("second", Some("blue"), 1),
            ("second", Some("red"), 1),
        ]
    );
    // The fan of the quad starts at its first corner; negative indices count from the end.
    let first = &read.scene.meshes[0].mesh;
    assert_eq!(first.triangles()[0], [0, 1, 2]);
    assert_eq!(first.triangles()[1], [0, 2, 3]);
    assert_eq!(first.triangle_count(), 5);
    let materials = &read.scene.materials;
    assert_eq!(materials.len(), 2);
    assert_eq!(materials[0].diffuse, Some([1.0, 0.0, 0.0]));
    assert_eq!(materials[0].opacity, Some(0.5));
    assert_eq!(materials[0].textures[0].slot, "map_Kd");
    assert_eq!(
        materials[0].textures[0].value,
        "-s 1 1 1 textures/red diffuse.png"
    );
    assert_eq!(materials[1].opacity, Some(0.75));
    assert!(materials[1].textures.is_empty());
    // Only polygons are triangulated, and the line statement is skipped and reported.
    assert_eq!(read.report.summary.approximated, 2);
    assert!(
        read.report
            .items
            .iter()
            .any(|i| i.reason == "io.mesh.element_skipped")
    );
}

#[test]
fn obj_rejects_bad_indices_and_short_faces() {
    let cases = [
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n",
            "io.mesh.index_out_of_range",
        ),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n",
            "io.mesh.index_out_of_range",
        ),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf -4 1 2\n",
            "io.mesh.index_out_of_range",
        ),
        ("v 0 0 0\nv 1 0 0\nf 1 2\n", "io.mesh.face_invalid"),
        (
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/2/3/4 2 3\n",
            "io.mesh.structure_invalid",
        ),
        ("v 0 0 nan\n", "io.mesh.value_invalid"),
        ("v 0 0\n", "io.mesh.structure_invalid"),
        ("usemtl\n", "io.mesh.structure_invalid"),
    ];
    for (text, code) in cases {
        let error = obj::read(text.as_bytes(), None, &mut ReadOptions::default()).unwrap_err();
        assert_eq!(error.code(), code, "{text:?}");
    }
}

#[test]
fn obj_continuation_lines_are_joined() {
    let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 \\\n 3\n";
    let read = read_ok(obj::read(
        text.as_bytes(),
        None,
        &mut ReadOptions::default(),
    ));
    assert_eq!(read.scene.meshes[0].mesh.triangle_count(), 1);
}

#[test]
fn stl_binary_is_chosen_by_length_not_by_the_header_word() {
    let mut binary = cube_stl_binary();
    binary[..5].copy_from_slice(b"solid");
    let read = read_ok(stl::read(&binary, &mut ReadOptions::default()));
    assert_eq!(read.scene.meshes[0].mesh.triangle_count(), 12);
    // One byte more than the facets need: not binary, so read as ASCII, which rejects the bytes.
    binary.push(0);
    let error = stl::read(&binary, &mut ReadOptions::default()).unwrap_err();
    assert!(
        ["io.mesh.text_invalid", "io.mesh.structure_invalid"].contains(&error.code()),
        "{}",
        error.code()
    );
}

#[test]
fn stl_ascii_solids_become_named_parts() {
    let text = "solid upper\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid upper\n\
solid lower\nfacet normal 0 0 -1\nouter loop\nvertex 0 0 0\nvertex 0 1 0\nvertex 1 0 0\nendloop\nendfacet\nendsolid lower\n";
    let read = read_ok(stl::read(text.as_bytes(), &mut ReadOptions::default()));
    let names: Vec<&str> = read.scene.meshes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["upper", "lower"]);
    assert_eq!(read.scene.meshes[0].mesh.vertex_count(), 3);
    assert_eq!(read.report.summary.dropped, 2, "two non-zero facet normals");
}

#[test]
fn stl_rejects_an_unfinished_solid() {
    let text = "solid open\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\n";
    let error = stl::read(text.as_bytes(), &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.mesh.truncated");
}

#[test]
fn ply_header_variants_and_skipped_parts() {
    let text = "ply\r\nformat ascii 1.0\r\ncomment made by a tool\r\nobj_info extra\r\n\
element vertex 4\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\n\
property float texture_u\r\nproperty float texture_v\r\nproperty uchar quality\r\n\
element material 1\r\nproperty list uchar float weights\r\n\
element face 1\r\nproperty list uchar uint vertex_index\r\nproperty uchar flags\r\nend_header\r\n\
0 0 0 0 0 7\n1 0 0 1 0 7\n1 1 0 1 1 7\n0 1 0 0 1 7\n\
2 0.5 0.25\n\
4 0 1 2 3 1\n";
    let read = read_ok(ply::read(text.as_bytes(), &mut ReadOptions::default()));
    assert_eq!(read.scene.meshes[0].mesh.triangle_count(), 2);
    assert!(read.scene.meshes[0].mesh.uv_set(0).is_some());
    let reasons: Vec<&str> = read
        .report
        .items
        .iter()
        .map(|i| i.reason.as_str())
        .collect();
    assert!(reasons.contains(&"io.mesh.property_skipped"));
    assert!(reasons.contains(&"io.mesh.element_skipped"));
    assert!(reasons.contains(&"io.mesh.polygon_triangulated"));
}

#[test]
fn ply_rejects_missing_position_and_unknown_format() {
    let no_x = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float y\nend_header\n0\n";
    assert_eq!(
        ply::read(no_x.as_bytes(), &mut ReadOptions::default())
            .unwrap_err()
            .code(),
        "io.mesh.structure_invalid"
    );
    let unknown = "ply\nformat ascii 2.0\nend_header\n";
    assert_eq!(
        ply::read(unknown.as_bytes(), &mut ReadOptions::default())
            .unwrap_err()
            .code(),
        "io.mesh.header_invalid"
    );
    let no_magic = "format ascii 1.0\nend_header\n";
    assert_eq!(
        ply::read(no_magic.as_bytes(), &mut ReadOptions::default())
            .unwrap_err()
            .code(),
        "io.mesh.header_invalid"
    );
}

#[test]
fn ply_face_index_checked_after_the_vertices_are_read() {
    let text = "ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n3 0 1 2\n0 0 0\n1 0 0\n0 1 0\n";
    let read = read_ok(ply::read(text.as_bytes(), &mut ReadOptions::default()));
    assert_eq!(read.scene.meshes[0].mesh.triangles(), &[[0, 1, 2]]);
}

fn scene_of(mesh: TriMesh) -> MeshScene {
    MeshScene {
        meshes: vec![MeshPart {
            name: String::new(),
            mesh,
            material: None,
            preserved: Default::default(),
        }],
        materials: Vec::new(),
        ..MeshScene::default()
    }
}

fn triangle_mesh(points: &[[f64; 3]], triangles: &[[u32; 3]]) -> TriMesh {
    let positions = points
        .iter()
        .map(|p| Point3::new(p[0], p[1], p[2]))
        .collect();
    TriMesh::new(positions, triangles.to_vec(), &Limits::DEFAULT).unwrap()
}

#[test]
fn stl_binary_rounds_positions_and_reports_names_and_parts() {
    let mesh = triangle_mesh(
        &[[0.1, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &[[0, 1, 2]],
    );
    let mut scene = scene_of(mesh.clone());
    scene.meshes[0].name = "named".to_owned();
    let written = stl::write(
        &scene,
        &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian),
    )
    .unwrap();
    assert_eq!(written.bytes.len(), 84 + 50);
    assert_eq!(
        written.report.summary.approximated, 1,
        "0.1 is not a single-precision value"
    );
    assert!(
        written
            .report
            .items
            .iter()
            .any(|i| i.reason == "io.mesh.attribute_dropped")
    );
    let mut two = scene.clone();
    two.meshes.push(two.meshes[0].clone());
    let combined = stl::write(
        &two,
        &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian),
    )
    .unwrap();
    assert!(
        combined
            .report
            .items
            .iter()
            .any(|i| i.record_kind.as_deref() == Some("part boundary"))
    );
}

#[test]
fn writers_refuse_encodings_the_format_does_not_have() {
    let scene = scene_of(triangle_mesh(
        &[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &[[0, 1, 2]],
    ));
    let error = obj::write(
        &scene,
        &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian),
    )
    .unwrap_err();
    assert_eq!(error.code(), "io.mesh.encoding_unsupported");
    let error = stl::write(
        &scene,
        &mut WriteOptions::with_encoding(Encoding::BinaryBigEndian),
    )
    .unwrap_err();
    assert_eq!(error.code(), "io.mesh.encoding_unsupported");
}

#[test]
fn obj_writer_names_the_material_library_and_round_trips_it() {
    let mesh = triangle_mesh(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[[0, 1, 2]]);
    let mut scene = scene_of(mesh);
    scene.meshes[0].name = "part".to_owned();
    scene.meshes[0].material = Some("paint".to_owned());
    scene.materials.push(arc_io_mesh::Material {
        name: "paint".to_owned(),
        diffuse: Some([0.25, 0.5, 0.75]),
        opacity: Some(0.5),
        textures: vec![arc_io_mesh::TextureMap {
            slot: "map_Kd".to_owned(),
            value: "C:\\maps\\paint.png".to_owned(),
        }],
        ..arc_io_mesh::Material::default()
    });
    let mut options = WriteOptions {
        mtl_name: Some("scene.mtl".to_owned()),
        ..WriteOptions::default()
    };
    let written = obj::write(&scene, &mut options).unwrap();
    let text = String::from_utf8(written.bytes.clone()).unwrap();
    assert!(text.starts_with("mtllib scene.mtl\n"));
    let mtl = written.mtl.clone().unwrap();
    let read = read_ok(obj::read(
        &written.bytes,
        Some(&mtl),
        &mut ReadOptions::default(),
    ));
    assert_eq!(read.scene.materials, scene.materials);
    assert_eq!(read.scene.meshes[0].material.as_deref(), Some("paint"));
    let again = obj::write(
        &read.scene,
        &mut WriteOptions {
            mtl_name: Some("scene.mtl".to_owned()),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        again.bytes, written.bytes,
        "writing is deterministic and stable"
    );
    assert_eq!(again.mtl, written.mtl);
}

#[test]
fn ply_writer_reports_dropped_names_and_rounded_colors() {
    let mut mesh = triangle_mesh(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[[0, 1, 2]]);
    let colors = vec![[0.1_f32, 0.2, 0.3, 1.0]; 3];
    mesh = TriMesh::new(
        mesh.positions().to_vec(),
        mesh.triangles().to_vec(),
        &Limits::DEFAULT,
    )
    .unwrap();
    let mut scene = scene_of(mesh);
    scene.meshes[0].name = "named".to_owned();
    let channel = arc_mesh::TriMeshBuilder::new(
        scene.meshes[0].mesh.positions().to_vec(),
        scene.meshes[0].mesh.triangles().to_vec(),
    )
    .channel(
        arc_mesh::AttributeKey::Color,
        arc_mesh::Channel::Vec4(colors),
    )
    .build(&Limits::DEFAULT)
    .unwrap();
    scene.meshes[0].mesh = channel;
    let written = ply::write(&scene, &mut WriteOptions::default()).unwrap();
    let text = String::from_utf8(written.bytes).unwrap();
    assert!(text.contains("property uchar red"));
    assert!(text.contains("element face 1"));
    assert!(
        written
            .report
            .items
            .iter()
            .any(|i| i.reason == "io.mesh.attribute_dropped")
    );
    assert!(
        written
            .report
            .items
            .iter()
            .any(|i| i.reason == "io.mesh.precision_reduced")
    );
}

#[test]
fn cancellation_stops_every_reader_and_writer() {
    let cancel = Cancel::new();
    cancel.cancel();
    let mut options = ReadOptions {
        cancel: cancel.clone(),
        ..ReadOptions::default()
    };
    assert_eq!(
        obj::read(CUBE_OBJ.as_bytes(), None, &mut options)
            .unwrap_err()
            .code(),
        "base.cancelled"
    );
    assert_eq!(
        stl::read(CUBE_STL.as_bytes(), &mut options)
            .unwrap_err()
            .code(),
        "base.cancelled"
    );
    assert_eq!(
        ply::read(CUBE_PLY.as_bytes(), &mut options)
            .unwrap_err()
            .code(),
        "base.cancelled"
    );
    let scene = scene_of(triangle_mesh(
        &[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &[[0, 1, 2]],
    ));
    let mut write = WriteOptions {
        cancel,
        ..WriteOptions::default()
    };
    assert_eq!(
        ply::write(&scene, &mut write).unwrap_err().code(),
        "base.cancelled"
    );
}

#[test]
fn obj_material_stays_in_effect_across_objects_and_groups() {
    let text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\n\
usemtl red\no first\nf 1 2 3\no second\nf 2 4 3\ng third\nf 1 4 2\n";
    let read = read_ok(obj::read(
        text.as_bytes(),
        None,
        &mut ReadOptions::default(),
    ));
    let parts: Vec<(&str, Option<&str>)> = read
        .scene
        .meshes
        .iter()
        .map(|p| (p.name.as_str(), p.material.as_deref()))
        .collect();
    assert_eq!(
        parts,
        vec![
            ("first", Some("red")),
            ("second", Some("red")),
            ("third", Some("red")),
        ]
    );
    // The written file names the material again after each object statement.
    let written = read_ok(obj::write(&read.scene, &mut WriteOptions::default()));
    let written = String::from_utf8(written.bytes).unwrap();
    assert_eq!(written.matches("usemtl red").count(), 3);
}

#[test]
fn obj_writer_reports_a_part_that_inherits_the_material_in_effect() {
    let square = triangle_mesh(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &[[0, 1, 2]],
    );
    let part = |name: &str, material: Option<&str>| MeshPart {
        name: name.to_owned(),
        mesh: square.clone(),
        material: material.map(str::to_owned),
        preserved: Default::default(),
    };
    let scene = MeshScene {
        meshes: vec![
            part("a", Some("red")),
            part("b", None),
            part("c", Some("blue")),
            part("d", None),
        ],
        materials: Vec::new(),
        ..MeshScene::default()
    };
    let written = read_ok(obj::write(&scene, &mut WriteOptions::default()));
    assert_eq!(written.report.summary.approximated, 2);
    let read = read_ok(obj::read(&written.bytes, None, &mut ReadOptions::default()));
    let materials: Vec<Option<&str>> = read
        .scene
        .meshes
        .iter()
        .map(|p| p.material.as_deref())
        .collect();
    assert_eq!(
        materials,
        vec![Some("red"), Some("red"), Some("blue"), Some("blue")]
    );
}
