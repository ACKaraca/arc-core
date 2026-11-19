//! Round trips: writing a scene and reading it back gives the same triangles, positions and
//! attributes, bit for bit, and writing is deterministic.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "test data"
)]

use arc_base::Limits;
use arc_base::math::Point3;
use arc_io_mesh::{Encoding, MeshPart, MeshScene, ReadOptions, WriteOptions, obj, ply, stl};
use arc_mesh::{AttributeKey, Channel, TriMesh, TriMeshBuilder};
use proptest::prelude::*;

/// Finite values of every class: normal, subnormal, zero, both signs.
fn finite_f64() -> impl Strategy<Value = f64> {
    proptest::num::f64::POSITIVE
        | proptest::num::f64::NEGATIVE
        | proptest::num::f64::NORMAL
        | proptest::num::f64::SUBNORMAL
        | proptest::num::f64::ZERO
}

fn finite_f32() -> impl Strategy<Value = f32> {
    proptest::num::f32::POSITIVE
        | proptest::num::f32::NEGATIVE
        | proptest::num::f32::NORMAL
        | proptest::num::f32::SUBNORMAL
        | proptest::num::f32::ZERO
}

/// Names that the formats can hold: lowercase words, possibly empty.
fn name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,6}|"
}

/// A part with its attributes. Colors are whole bytes, which the PLY colors hold exactly.
#[derive(Debug, Clone)]
struct PartSpec {
    name: String,
    material: Option<String>,
    points: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[u8; 4]>>,
}

/// A part whose positions are single-precision values when `single` is set, so STL keeps them.
fn part_spec(single: bool) -> impl Strategy<Value = PartSpec> {
    let position: BoxedStrategy<f64> = if single {
        finite_f32().prop_map(f64::from).boxed()
    } else {
        finite_f64().boxed()
    };
    (1_usize..9).prop_flat_map(move |count| {
        let n = count as u32;
        (
            name(),
            proptest::option::of(name()),
            prop::collection::vec(prop::array::uniform3(position.clone()), count),
            prop::collection::vec(prop::array::uniform3(0..n), 1..8),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform3(finite_f32()),
                count,
            )),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform2(finite_f32()),
                count,
            )),
            proptest::option::of(prop::collection::vec(
                prop::array::uniform4(any::<u8>()),
                count,
            )),
        )
            .prop_map(
                |(name, material, points, triangles, normals, uvs, colors)| PartSpec {
                    name,
                    material: material.filter(|m| !m.is_empty()),
                    points,
                    triangles,
                    normals,
                    uvs,
                    colors,
                },
            )
    })
}

fn build(spec: &PartSpec) -> MeshPart {
    let positions = spec
        .points
        .iter()
        .map(|p| Point3::new(p[0], p[1], p[2]))
        .collect();
    let mut builder = TriMeshBuilder::new(positions, spec.triangles.clone());
    if let Some(values) = &spec.normals {
        builder = builder.channel(AttributeKey::Normal, Channel::Vec3(values.clone()));
    }
    if let Some(values) = &spec.uvs {
        builder = builder.channel(AttributeKey::Uv(0), Channel::Vec2(values.clone()));
    }
    if let Some(values) = &spec.colors {
        let colors = values
            .iter()
            .map(|c| c.map(|byte| (f64::from(byte) / 255.0) as f32))
            .collect();
        builder = builder.channel(AttributeKey::Color, Channel::Vec4(colors));
    }
    MeshPart {
        name: spec.name.clone(),
        mesh: builder.build(&Limits::DEFAULT).unwrap(),
        material: spec.material.clone(),
        preserved: Default::default(),
    }
}

fn scene(parts: &[PartSpec]) -> MeshScene {
    MeshScene {
        meshes: parts.iter().map(build).collect(),
        materials: Vec::new(),
        ..MeshScene::default()
    }
}

/// Which attributes a format keeps, for the comparison.
#[derive(Clone, Copy)]
struct Keep {
    normal: bool,
    uv: bool,
    color: bool,
}

/// The attribute values of each corner of a mesh, in triangle order, bit for bit.
fn corners(mesh: &TriMesh, keep: Keep) -> Vec<Vec<u64>> {
    let at = |index: u32| {
        let i = usize::try_from(index).unwrap();
        let mut key = Vec::new();
        if let Some(p) = mesh.positions().get(i) {
            key.extend([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]);
        }
        if keep.normal
            && let Some(n) = mesh.normals().and_then(|v| v.get(i))
        {
            key.extend(n.map(|c| u64::from(c.to_bits())));
        }
        if keep.uv
            && let Some(uv) = mesh.uv_set(0).and_then(|v| v.get(i))
        {
            key.extend(uv.map(|c| u64::from(c.to_bits())));
        }
        if keep.color
            && let Some(Channel::Vec4(values)) = mesh.channel(&AttributeKey::Color)
            && let Some(c) = values.get(i)
        {
            key.extend(c.map(|v| u64::from(v.to_bits())));
        }
        key
    };
    mesh.triangles()
        .iter()
        .flat_map(|t| t.iter().map(|&i| at(i)))
        .collect()
}

fn positions_only() -> Keep {
    Keep {
        normal: false,
        uv: false,
        color: false,
    }
}

fn read_obj(bytes: &[u8]) -> MeshScene {
    obj::read(bytes, None, &mut ReadOptions::default())
        .unwrap()
        .scene
}

proptest! {
    #[test]
    fn obj_round_trip_is_bit_exact_and_deterministic(
        parts in prop::collection::vec(part_spec(false), 1..4)
    ) {
        let scene = scene(&parts);
        let written = obj::write(&scene, &mut WriteOptions::default()).unwrap();
        let again = obj::write(&scene, &mut WriteOptions::default()).unwrap();
        prop_assert_eq!(&written.bytes, &again.bytes);
        let read = read_obj(&written.bytes);
        prop_assert_eq!(read.meshes.len(), scene.meshes.len());
        let keep = Keep { normal: true, uv: true, color: false };
        // A usemtl stays in effect across o and g, so a part without a material reads back with
        // the material in effect after the parts before it.
        let mut in_effect: Option<String> = None;
        for (original, got) in scene.meshes.iter().zip(&read.meshes) {
            prop_assert_eq!(&original.name, &got.name);
            if original.material.is_some() {
                in_effect.clone_from(&original.material);
            }
            prop_assert_eq!(&in_effect, &got.material);
            prop_assert_eq!(corners(&original.mesh, keep), corners(&got.mesh, keep));
        }
    }

    #[test]
    fn stl_binary_round_trip_is_bit_exact_for_single_precision_positions(
        part in part_spec(true)
    ) {
        let scene = scene(std::slice::from_ref(&part));
        let written = stl::write(&scene, &mut WriteOptions::with_encoding(Encoding::BinaryLittleEndian)).unwrap();
        prop_assert_eq!(written.report.summary.approximated, 0);
        let read = stl::read(&written.bytes, &mut ReadOptions::default()).unwrap().scene;
        let expected = corners(&scene.meshes[0].mesh, positions_only());
        let got = read.meshes.first().map(|p| corners(&p.mesh, positions_only())).unwrap_or_default();
        prop_assert_eq!(expected, got);
    }

    #[test]
    fn stl_ascii_round_trip_keeps_solids_and_positions(
        parts in prop::collection::vec(part_spec(true), 1..4)
    ) {
        let scene = scene(&parts);
        let written = stl::write(&scene, &mut WriteOptions::default()).unwrap();
        let read = stl::read(&written.bytes, &mut ReadOptions::default()).unwrap().scene;
        prop_assert_eq!(read.meshes.len(), scene.meshes.len());
        for (original, got) in scene.meshes.iter().zip(&read.meshes) {
            prop_assert_eq!(&original.name, &got.name);
            prop_assert_eq!(corners(&original.mesh, positions_only()), corners(&got.mesh, positions_only()));
        }
    }

    #[test]
    fn ply_round_trip_is_exact_in_every_encoding(
        part in part_spec(false),
        encoding in prop_oneof![
            Just(Encoding::Ascii),
            Just(Encoding::BinaryLittleEndian),
            Just(Encoding::BinaryBigEndian),
        ],
    ) {
        let mut part = part;
        part.name = String::new();
        part.material = None;
        let scene = scene(std::slice::from_ref(&part));
        let written = ply::write(&scene, &mut WriteOptions::with_encoding(encoding)).unwrap();
        let again = ply::write(&scene, &mut WriteOptions::with_encoding(encoding)).unwrap();
        prop_assert_eq!(&written.bytes, &again.bytes);
        let read = ply::read(&written.bytes, &mut ReadOptions::default())
            .map_err(|e| TestCaseError::fail(format!("{encoding:?} {} {:?} {:?}", e.code(), e.details(), part)))?
            .scene;
        prop_assert_eq!(read.meshes.len(), 1);
        let got = &read.meshes[0].mesh;
        prop_assert_eq!(got.triangles(), scene.meshes[0].mesh.triangles());
        let keep = Keep { normal: part.normals.is_some(), uv: part.uvs.is_some(), color: part.colors.is_some() };
        prop_assert_eq!(corners(&scene.meshes[0].mesh, keep), corners(got, keep));
        prop_assert_eq!(written.report.summary.approximated, 0);
    }
}

#[test]
fn ply_colors_are_rounded_and_reported_when_they_are_not_bytes() {
    let mesh = TriMeshBuilder::new(
        vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ],
        vec![[0, 1, 2]],
    )
    .channel(
        AttributeKey::Color,
        Channel::Vec4(vec![[0.123, 0.5, 0.0, 1.0]; 3]),
    )
    .build(&Limits::DEFAULT)
    .unwrap();
    let scene = MeshScene {
        meshes: vec![MeshPart {
            name: String::new(),
            mesh,
            material: None,
            preserved: Default::default(),
        }],
        materials: Vec::new(),
        ..MeshScene::default()
    };
    let written = ply::write(&scene, &mut WriteOptions::default()).unwrap();
    assert_eq!(written.report.summary.approximated, 3);
    let read = ply::read(&written.bytes, &mut ReadOptions::default())
        .unwrap()
        .scene;
    let Some(Channel::Vec4(colors)) = read.meshes[0].mesh.channel(&AttributeKey::Color) else {
        panic!("color channel lost");
    };
    assert!((colors[0][0] - 0.123).abs() < 1.0 / 255.0);
    assert_eq!(colors[0][1].to_bits(), (128.0_f32 / 255.0).to_bits());
}
