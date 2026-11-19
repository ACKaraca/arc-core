//! glTF and GLB: hand-written fixtures, the error paths, hostile input and round trips. No sample
//! file is used; every document is written in the test.

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

use arc_base::math::Point3;
use arc_base::{Error, Limits};
use arc_io_mesh::gltf;
use arc_io_mesh::{
    AlphaMode, Encoding, ImageSource, Material, MeshGroup, MeshPart, MeshScene, Node, ReadOptions,
    ReadOutput, Transform, WriteOptions,
};
use arc_mesh::{AttributeKey, Channel, TriMeshBuilder};
use proptest::prelude::*;

const TRIANGLE_URI: &str =
    "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA";

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A GLB container with the JSON and binary chunks, padded as the format requires.
fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
    out.extend(2_u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534A_u32.to_le_bytes());
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004E_4942_u32.to_le_bytes());
    out.extend(bin);
    out
}

fn read(bytes: &[u8]) -> arc_base::Result<ReadOutput> {
    gltf::read(bytes, |_| None, &mut ReadOptions::default())
}

fn read_ok(bytes: &[u8]) -> ReadOutput {
    read(bytes).unwrap()
}

fn code(result: arc_base::Result<ReadOutput>) -> &'static str {
    result.err().map_or("ok", |error: Error| error.code())
}

/// The triangle of the fixtures: three positions and the indices 0, 1, 2.
const TRIANGLE_POSITIONS: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

#[test]
fn triangle_json_with_a_data_uri_reads_the_part() {
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
        "buffers":[{{"byteLength":42,"uri":"{TRIANGLE_URI}"}}],
        "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":6}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}},
                     {{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}],
        "meshes":[{{"name":"tri","primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],
        "nodes":[{{"mesh":0}}],
        "scenes":[{{"nodes":[0]}}],"scene":0}}"#
    );
    let read = read_ok(json.as_bytes());
    assert_eq!(read.scene.roots, vec![0]);
    assert_eq!(read.scene.mesh_groups[0].name, "tri");
    let part = &read.scene.meshes[0];
    assert_eq!(part.mesh.triangles(), &[[0, 1, 2]]);
    assert_eq!(part.mesh.positions()[1], Point3::new(1.0, 0.0, 0.0));
    assert!(read.report.summary.dropped == 0);
}

#[test]
fn the_same_triangle_as_a_glb_built_by_the_test_reads_identically() {
    let json = r#"{"asset":{"version":"2.0"},
        "buffers":[{"byteLength":42}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                     {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
        "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}"#;
    let mut bin = f32_bytes(&TRIANGLE_POSITIONS);
    bin.extend([0_u8, 0, 1, 0, 2, 0]);
    let from_glb = read_ok(&glb(json, &bin));
    let from_uri = read_ok(TRIANGLE_URI_JSON.as_bytes());
    assert_eq!(from_glb.scene, from_uri.scene);
}

const TRIANGLE_URI_JSON: &str = r#"{"asset":{"version":"2.0"},
    "buffers":[{"byteLength":42,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA"}],
    "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
    "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                 {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
    "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
    "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}"#;

#[test]
fn two_nodes_sharing_one_mesh_store_its_parts_once() {
    let json = r#"{"asset":{"version":"2.0"},
        "buffers":[{"byteLength":42,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAABAAIA"}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":6}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                     {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
        "nodes":[{"mesh":0,"translation":[5,0,0]},{"mesh":0,"translation":[-5,0,0]}],
        "scenes":[{"nodes":[0,1]}]}"#;
    let read = read_ok(json.as_bytes());
    assert_eq!(read.scene.meshes.len(), 1);
    assert_eq!(read.scene.mesh_groups.len(), 1);
    assert_eq!(read.scene.roots, vec![0, 1]);
    assert_eq!(read.scene.nodes[0].mesh, Some(0));
    assert_eq!(read.scene.nodes[1].mesh, Some(0));
    assert_eq!(
        read.scene.nodes[1].transform,
        Transform::Trs {
            translation: [-5.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    );
}

#[test]
fn sparse_accessor_replaces_the_listed_elements() {
    let mut bin = vec![0_u8; 36];
    bin.push(1);
    bin.extend([0_u8; 3]);
    bin.extend(f32_bytes(&[5.0, 6.0, 7.0]));
    let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":52}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":1},{"buffer":0,"byteOffset":40,"byteLength":12}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
            "sparse":{"count":1,"indices":{"bufferView":1,"componentType":5121},"values":{"bufferView":2}}}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
        "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}"#;
    let read = read_ok(&glb(json, &bin));
    let positions = read.scene.meshes[0].mesh.positions();
    assert_eq!(positions[0], Point3::new(0.0, 0.0, 0.0));
    assert_eq!(positions[1], Point3::new(5.0, 6.0, 7.0));
    assert_eq!(positions[2], Point3::new(0.0, 0.0, 0.0));
}

#[test]
fn strided_buffer_view_reads_interleaved_attributes() {
    // Three vertices of position then normal, 24 bytes apart.
    let mut bin = Vec::new();
    for vertex in 0..3 {
        let base = vertex as f32;
        bin.extend(f32_bytes(&[base, base + 0.5, 0.0, 0.0, 0.0, 1.0]));
    }
    let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":72}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":72,"byteStride":24}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                     {"bufferView":0,"byteOffset":12,"componentType":5126,"count":3,"type":"VEC3"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1}}]}],
        "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}"#;
    let read = read_ok(&glb(json, &bin));
    let mesh = &read.scene.meshes[0].mesh;
    assert_eq!(mesh.positions()[2], Point3::new(2.0, 2.5, 0.0));
    assert_eq!(mesh.normals().unwrap()[1], [0.0, 0.0, 1.0]);
}

#[test]
fn normalized_u8_colors_are_scaled_to_the_unit_range() {
    let mut bin = f32_bytes(&TRIANGLE_POSITIONS);
    bin.extend([255_u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 128]);
    let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":48}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":12}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                     {"bufferView":1,"componentType":5121,"normalized":true,"count":3,"type":"VEC4"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"COLOR_0":1}}]}],
        "nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}]}"#;
    let read = read_ok(&glb(json, &bin));
    let colors = read.scene.meshes[0].mesh.colors().unwrap();
    assert_eq!(colors[0], [1.0, 0.0, 0.0, 1.0]);
    assert!((colors[2][3] - 128.0 / 255.0).abs() < 1e-6);
}

#[test]
fn strip_and_fan_become_triangles_and_points_are_skipped() {
    let indices = [0_u8, 1, 2, 3];
    let mut bin = f32_bytes(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
    bin.extend(indices);
    let json = |mode: u32| {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":52}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":48}},{{"buffer":0,"byteOffset":48,"byteLength":4}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
                         {{"bufferView":1,"componentType":5121,"count":4,"type":"SCALAR"}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"mode":{mode}}}]}}],
            "nodes":[{{"mesh":0}}],"scenes":[{{"nodes":[0]}}]}}"#
        )
    };
    let strip = read_ok(&glb(&json(5), &bin));
    assert_eq!(
        strip.scene.meshes[0].mesh.triangles(),
        &[[0, 1, 2], [2, 1, 3]]
    );
    let fan = read_ok(&glb(&json(6), &bin));
    assert_eq!(
        fan.scene.meshes[0].mesh.triangles(),
        &[[0, 1, 2], [0, 2, 3]]
    );
    let points = read_ok(&glb(&json(0), &bin));
    assert!(points.scene.meshes.is_empty());
    assert!(points.report.has_loss());
}

#[test]
fn hierarchy_errors_and_unsupported_required_extensions_are_refused() {
    let cycle = r#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{"children":[0]}],"scenes":[{"nodes":[0]}]}"#;
    assert_eq!(code(read(cycle.as_bytes())), "io.mesh.reference_invalid");
    let twice = r#"{"asset":{"version":"2.0"},"nodes":[{}],"scenes":[{"nodes":[0,0]}]}"#;
    assert_eq!(code(read(twice.as_bytes())), "io.mesh.reference_invalid");
    let missing =
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[7]}],"scenes":[{"nodes":[0]}]}"#;
    assert_eq!(code(read(missing.as_bytes())), "io.mesh.reference_invalid");
    let draco =
        r#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#;
    assert_eq!(
        code(read(draco.as_bytes())),
        "io.mesh.extension_unsupported"
    );
    let version = r#"{"asset":{"version":"1.0"}}"#;
    assert_eq!(code(read(version.as_bytes())), "io.mesh.header_invalid");
    let mut bad_glb = glb(r#"{"asset":{"version":"2.0"}}"#, &[]);
    bad_glb[4] = 1;
    assert_eq!(code(read(&bad_glb)), "io.mesh.header_invalid");
    let external =
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"missing.bin"}]}"#;
    assert_eq!(
        code(read(external.as_bytes())),
        "io.mesh.resource_unavailable"
    );
}

#[test]
fn unknown_extensions_and_extras_are_kept_as_text_and_reported() {
    let json = r#"{"asset":{"version":"2.0"},
        "nodes":[{"name":"a","extensions":{"VENDOR_thing":{"z":1,"a":[true]}},"extras":{"k":"v"}}],
        "scenes":[{"nodes":[0]}]}"#;
    let read = read_ok(json.as_bytes());
    let node = &read.scene.nodes[0];
    assert_eq!(
        node.preserved.extensions.as_deref(),
        Some(r#"{"VENDOR_thing":{"a":[true],"z":1}}"#)
    );
    assert_eq!(node.preserved.extras.as_deref(), Some(r#"{"k":"v"}"#));
    assert_eq!(read.report.summary.preserved, 2);
    assert!(
        read.report
            .items
            .iter()
            .any(|item| item.reason == "io.mesh.extension_preserved")
    );
}

#[test]
fn node_depth_is_bounded_by_the_limits_profile() {
    let mut json = String::from(r#"{"asset":{"version":"2.0"},"nodes":["#);
    for index in 0..20 {
        json.push_str(&format!(r#"{{"children":[{}]}},"#, index + 1));
    }
    json.push_str(r#"{}],"scenes":[{"nodes":[0]}]}"#);
    let limits = Limits {
        nesting_depth: 8,
        ..Limits::CONSTRAINED
    };
    let result = gltf::read(
        json.as_bytes(),
        |_| None,
        &mut ReadOptions::with_limits(limits),
    );
    let error = result.unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(format!("{:?}", error.arguments()).contains("nesting_depth"));
}

#[test]
fn separate_buffer_is_returned_beside_the_json_and_read_through_the_resolver() {
    let scene = triangle_scene(TRIANGLE_POSITIONS.to_vec(), None);
    let mut options = WriteOptions {
        buffer_name: Some("triangle.bin".to_owned()),
        ..WriteOptions::default()
    };
    let written = gltf::write(&scene, &mut options).unwrap();
    let buffer = written.buffer.clone().unwrap();
    assert!(written.bytes.windows(12).any(|w| w == b"triangle.bin"));
    let read = gltf::read(
        &written.bytes,
        |uri| (uri == "triangle.bin").then(|| buffer.clone()),
        &mut ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(read.scene, scene);
}

#[test]
fn writer_refuses_encodings_and_references_gltf_cannot_hold() {
    let scene = triangle_scene(TRIANGLE_POSITIONS.to_vec(), None);
    let refused = gltf::write(
        &scene,
        &mut WriteOptions::with_encoding(Encoding::BinaryBigEndian),
    );
    assert_eq!(refused.unwrap_err().code(), "io.mesh.encoding_unsupported");
    let mut broken = scene.clone();
    broken.meshes[0].material = Some("nothing".to_owned());
    let result = gltf::write(&broken, &mut WriteOptions::default());
    assert_eq!(result.unwrap_err().code(), "io.mesh.write_value_invalid");
}

/// A scene of one part with the given positions and an optional material, drawn by one node.
fn triangle_scene(positions: Vec<f32>, material: Option<&str>) -> MeshScene {
    let points = positions
        .chunks_exact(3)
        .map(|p| Point3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])))
        .collect();
    let mesh = TriMeshBuilder::new(points, vec![[0, 1, 2]])
        .build(&Limits::DEFAULT)
        .unwrap();
    MeshScene {
        meshes: vec![MeshPart {
            name: String::new(),
            mesh,
            material: material.map(str::to_owned),
            preserved: Default::default(),
        }],
        mesh_groups: vec![MeshGroup {
            name: "tri".to_owned(),
            primitives: vec![0],
            preserved: Default::default(),
        }],
        nodes: vec![Node {
            mesh: Some(0),
            ..Node::default()
        }],
        roots: vec![0],
        materials: material
            .map(|name| Material {
                name: name.to_owned(),
                ..Material::default()
            })
            .into_iter()
            .collect(),
        ..MeshScene::default()
    }
}

/// Finite `f32` values of every class, as glTF stores them.
fn finite_f32() -> impl Strategy<Value = f32> {
    proptest::num::f32::POSITIVE
        | proptest::num::f32::NEGATIVE
        | proptest::num::f32::NORMAL
        | proptest::num::f32::SUBNORMAL
        | proptest::num::f32::ZERO
}

fn name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,5}|"
}

/// A scene that glTF holds exactly: one group, one node and one part per mesh, with normals,
/// texture coordinates, colors, a transform and optional PBR material fields.
fn gltf_scene() -> impl Strategy<Value = MeshScene> {
    let part = (1_usize..6).prop_flat_map(|count| {
        let n = count as u32;
        (
            prop::collection::vec(prop::array::uniform3(finite_f32()), count),
            prop::collection::vec(prop::array::uniform3(0..n), 1..6),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform3(finite_f32()),
                count,
            )),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform2(finite_f32()),
                count,
            )),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform4(finite_f32()),
                count,
            )),
        )
    });
    (
        prop::collection::vec(
            (
                part,
                name(),
                proptest::option::of(0_usize..2),
                prop::array::uniform3(finite_f32()),
            ),
            1..4,
        ),
        any::<bool>(),
    )
        .prop_map(|(parts, double_sided)| {
            let mut scene = MeshScene::default();
            for (
                index,
                ((points, triangles, normals, uvs, colors), group_name, material, translation),
            ) in parts.into_iter().enumerate()
            {
                let positions: Vec<Point3> = points
                    .iter()
                    .map(|p| Point3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])))
                    .collect();
                let mut builder = TriMeshBuilder::new(positions, triangles.to_vec());
                if let Some(normals) = normals {
                    builder = builder.channel(AttributeKey::Normal, Channel::Vec3(normals));
                }
                if let Some(uvs) = uvs {
                    builder = builder.channel(AttributeKey::Uv(0), Channel::Vec2(uvs));
                }
                if let Some(colors) = colors {
                    builder = builder.channel(AttributeKey::Color, Channel::Vec4(colors));
                }
                let mesh = builder.build(&Limits::DEFAULT).unwrap();
                scene.meshes.push(MeshPart {
                    name: String::new(),
                    mesh,
                    material: material.map(|m| format!("m{m}")),
                    preserved: Default::default(),
                });
                scene.mesh_groups.push(MeshGroup {
                    name: group_name,
                    primitives: vec![index],
                    preserved: Default::default(),
                });
                scene.nodes.push(Node {
                    name: format!("n{index}"),
                    transform: Transform::Trs {
                        translation,
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                    mesh: Some(index),
                    ..Node::default()
                });
                scene.roots.push(index);
            }
            for (index, name) in ["m0", "m1"].iter().enumerate() {
                scene.materials.push(Material {
                    name: (*name).to_owned(),
                    base_color_factor: Some([0.5, 0.25, 1.0, 0.75]),
                    metallic_factor: (index == 0).then_some(0.5),
                    roughness_factor: Some(0.125),
                    base_color_texture: (index == 1)
                        .then(|| ImageSource::Uri("albedo.png".to_owned())),
                    alpha_mode: Some(if index == 0 {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Mask
                    }),
                    alpha_cutoff: (index == 1).then_some(0.5),
                    double_sided,
                    ..Material::default()
                });
            }
            scene
        })
}

proptest! {
    #[test]
    fn write_then_read_returns_the_same_scene_as_glb_and_as_json(scene in gltf_scene()) {
        let glb_out = gltf::write(&scene, &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian)).unwrap();
        let back = gltf::read(&glb_out.bytes, |_| None, &mut ReadOptions::default()).unwrap();
        prop_assert_eq!(&back.scene, &scene);

        let json_out = gltf::write(&scene, &mut WriteOptions::default()).unwrap();
        let is_object = json_out.bytes.first() == Some(&b'{');
        prop_assert!(is_object);
        let back = gltf::read(&json_out.bytes, |_| None, &mut ReadOptions::default()).unwrap();
        prop_assert_eq!(&back.scene, &scene);
        prop_assert_eq!(gltf::write(&scene, &mut WriteOptions::default()).unwrap().bytes, json_out.bytes);
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512), glb_prefix in any::<bool>()) {
        let mut input = if glb_prefix { b"glTF".to_vec() } else { Vec::new() };
        input.extend(bytes);
        let _ = gltf::read(&input, |_| None, &mut ReadOptions::default());
    }

    #[test]
    fn mutated_valid_glb_and_json_never_panic(
        scene in gltf_scene(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        cut in any::<prop::sample::Index>(),
        as_glb in any::<bool>(),
    ) {
        let options = if as_glb {
            WriteOptions::with_encoding(Encoding::BinaryLittleEndian)
        } else {
            WriteOptions::default()
        };
        let mut options = options;
        let mut bytes = gltf::write(&scene, &mut options).unwrap().bytes;
        for (position, value) in &edits {
            let at = position.index(bytes.len());
            bytes[at] = *value;
        }
        let keep = cut.index(bytes.len() + 1);
        bytes.truncate(keep);
        let _ = gltf::read(&bytes, |_| None, &mut ReadOptions::default());
    }
}

#[test]
fn a_declared_length_larger_than_the_file_is_refused_at_once() {
    let started = Instant::now();
    let mut header = b"glTF".to_vec();
    header.extend(2_u32.to_le_bytes());
    header.extend(u32::MAX.to_le_bytes());
    header.extend([0_u8; 8]);
    assert_eq!(code(read(&header)), "io.mesh.truncated");

    let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":1000000,"uri":"data:application/octet-stream;base64,AAAA"}]}"#;
    assert_eq!(code(read(json.as_bytes())), "io.mesh.truncated");

    let huge = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}],
        "bufferViews":[{"buffer":0,"byteLength":36}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":9000000,"type":"VEC3"}]}"#;
    assert_eq!(code(read(huge.as_bytes())), "io.mesh.truncated");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_shared_mesh_is_written_once_and_named_by_both_nodes() {
    let mut scene = triangle_scene(TRIANGLE_POSITIONS.to_vec(), None);
    scene.nodes.push(Node {
        mesh: Some(0),
        transform: Transform::Trs {
            translation: [2.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        ..Node::default()
    });
    scene.roots.push(1);
    let written = gltf::write(&scene, &mut WriteOptions::default()).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&written.bytes).unwrap();
    assert_eq!(document["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(document["nodes"][0]["mesh"], 0);
    assert_eq!(document["nodes"][1]["mesh"], 0);
    assert_eq!(read_ok(&written.bytes).scene, scene);
}

#[test]
fn material_names_stay_unique_and_generated_names_do_not_collide() {
    let json = r#"{"asset":{"version":"2.0"},"materials":[{"name":"a"},{"name":"a"},{"name":"material-1"},{}]}"#;
    let read = read_ok(json.as_bytes());
    let names: Vec<&str> = read
        .scene
        .materials
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, vec!["a", "material-1", "material-2", "material-3"]);
}

#[test]
fn primitives_sharing_one_zero_filled_accessor_are_bounded_by_decompressed_bytes() {
    // One VEC3 accessor without a buffer view (zero-filled), shared by 10,000 primitives. Each
    // primitive holds about 2.4 MB of positions, so the total passes 16 MiB after seven of them.
    let primitives = vec![r#"{"attributes":{"POSITION":0}}"#; 10_000].join(",");
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"accessors":[{{"componentType":5126,"count":99999,"type":"VEC3"}}],
        "meshes":[{{"primitives":[{primitives}]}}],"nodes":[{{"mesh":0}}],"scenes":[{{"nodes":[0]}}]}}"#
    );
    let limits = Limits {
        decompressed_bytes: 16 * 1024 * 1024,
        ..Limits::CONSTRAINED
    };
    let error = gltf::read(
        json.as_bytes(),
        |_| None,
        &mut ReadOptions::with_limits(limits),
    )
    .unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(format!("{:?}", error.arguments()).contains("decompressed_bytes"));
}

#[test]
fn an_embedded_buffer_longer_than_string_bytes_reads_back() {
    let scene = triangle_scene(TRIANGLE_POSITIONS.to_vec(), None);
    let written = gltf::write(&scene, &mut WriteOptions::default()).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&written.bytes).unwrap();
    let uri = document["buffers"][0]["uri"].as_str().unwrap();
    let limits = Limits {
        string_bytes: 64,
        ..Limits::CONSTRAINED
    };
    assert!(uri.len() > 64);
    let read = gltf::read(
        &written.bytes,
        |_| None,
        &mut ReadOptions::with_limits(limits),
    )
    .unwrap();
    assert_eq!(read.scene, scene);
}

/// The component type of the index accessor of a one-primitive scene of `vertices` vertices, whose
/// last triangle refers to the last vertex.
fn index_component_type(vertices: usize) -> u64 {
    let points = (0..vertices)
        .map(|i| Point3::new(i as f64, 0.0, 0.0))
        .collect();
    let last = u32::try_from(vertices - 1).unwrap();
    let mesh = TriMeshBuilder::new(points, vec![[0, 1, last]])
        .build(&Limits::DEFAULT)
        .unwrap();
    let scene = MeshScene {
        meshes: vec![MeshPart {
            name: String::new(),
            mesh,
            material: None,
            preserved: Default::default(),
        }],
        mesh_groups: vec![MeshGroup {
            name: String::new(),
            primitives: vec![0],
            preserved: Default::default(),
        }],
        nodes: vec![Node {
            mesh: Some(0),
            ..Node::default()
        }],
        roots: vec![0],
        ..MeshScene::default()
    };
    let written = gltf::write(&scene, &mut WriteOptions::default()).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&written.bytes).unwrap();
    let index = document["meshes"][0]["primitives"][0]["indices"]
        .as_u64()
        .unwrap();
    document["accessors"][index as usize]["componentType"]
        .as_u64()
        .unwrap()
}

#[test]
fn indices_are_unsigned_shorts_only_when_no_index_is_65535() {
    assert_eq!(index_component_type(65_535), 5123);
    assert_eq!(index_component_type(65_536), 5125);
}

#[test]
fn a_number_that_is_not_finite_as_f32_is_refused() {
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{"translation":[1e39,0,0]}],"scenes":[{"nodes":[0]}]}"#;
    assert_eq!(code(read(json.as_bytes())), "io.mesh.value_invalid");
}

#[test]
fn a_cycle_outside_the_default_scene_is_refused_and_an_unreached_tree_is_kept() {
    let cycle = r#"{"asset":{"version":"2.0"},"nodes":[{},{"children":[2]},{"children":[1]}],"scenes":[{"nodes":[0]}]}"#;
    assert_eq!(code(read(cycle.as_bytes())), "io.mesh.reference_invalid");
    let tree =
        r#"{"asset":{"version":"2.0"},"nodes":[{},{"children":[2]},{}],"scenes":[{"nodes":[0]}]}"#;
    let read = read_ok(tree.as_bytes());
    assert_eq!(read.scene.roots, vec![0]);
    assert_eq!(read.scene.nodes.len(), 3);
}
