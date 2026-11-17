//! Golden vectors of the mesh encoding (CORE-MESH-002): the exact bytes of one exact and two
//! quantized meshes, and the decoded bits of the quantized mesh whose values lie off the grid,
//! so that any change of the byte layout or of the quantization arithmetic fails here. Changing a vector means a
//! new encoding version (docs/architecture/mesh-encoding.md).

#![allow(
    clippy::unwrap_used,
    reason = "the vectors are built from valid literal values"
)]

use arc_base::id::Identifier;
use arc_base::math::Point3;
use arc_base::{Limits, ObjectId};
use arc_mesh::{
    AttributeKey, Channel, FaceGroup, Quantization, SubObjectKey, TriMesh, TriMeshBuilder,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn triangle() -> TriMesh {
    TriMeshBuilder::new(
        vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 2.0, 0.0),
        ],
        vec![[0, 1, 2]],
    )
    .channel(
        AttributeKey::Normal,
        Channel::Vec3(vec![[0.0, 0.0, 1.0]; 3]),
    )
    .channel(
        AttributeKey::Tangent,
        Channel::Vec4(vec![[1.0, 0.0, 0.0, -1.0]; 3]),
    )
    .channel(
        AttributeKey::Uv(1),
        Channel::Vec2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
    )
    .channel(
        AttributeKey::Color,
        Channel::Vec4(vec![[1.0, 0.5, 0.25, 1.0]; 3]),
    )
    .channel(
        AttributeKey::User("w".into()),
        Channel::Scalar(vec![0.0, 0.5, 1.0]),
    )
    .group(FaceGroup {
        start: 0,
        end: 1,
        material: 2,
        key: SubObjectKey {
            object: Some(ObjectId::from_u128(
                0x0123_4567_89ab_7def_8123_4567_89ab_cdef,
            )),
            local: 5,
        },
    })
    .build(&Limits::STRICT)
    .unwrap()
}

#[test]
fn exact_mesh() {
    let mesh = triangle();
    let bytes = mesh.encode(&Quantization::default()).unwrap();
    assert_eq!(hex(&bytes), EXACT);
    assert_eq!(TriMesh::decode(&bytes, &Limits::STRICT).unwrap(), mesh);
}

/// Positions off every grid line and oblique normals, so the vector fixes the rounding of the
/// encoder and the decoder's order of operations, not only the layout.
fn oblique() -> TriMesh {
    TriMeshBuilder::new(
        vec![
            Point3::new(0.1, -0.7, 2.3),
            Point3::new(3.7, 0.25, -1.9),
            Point3::new(-2.2, 1.6, 0.05),
            Point3::new(0.9, 3.1, 1.15),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
    .channel(
        AttributeKey::Normal,
        Channel::Vec3(vec![
            [0.267_261_24, 0.534_522_5, 0.801_783_7],
            [-0.6, 0.0, -0.8],
            [0.123, -0.456, -0.789],
            [-0.3, -0.3, 0.1],
        ]),
    )
    .build(&Limits::STRICT)
    .unwrap()
}

#[test]
fn quantized_oblique_mesh() {
    let mesh = oblique();
    let quantization = Quantization {
        position_error: Some(0.01),
        normal_angle: Some(0.002),
    };
    let bytes = mesh.encode(&quantization).unwrap();
    let decoded = TriMesh::decode(&bytes, &Limits::STRICT).unwrap();
    let position_bits: Vec<[u64; 3]> = decoded
        .positions()
        .iter()
        .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
        .collect();
    let normal_bits: Vec<[u32; 3]> = decoded
        .normals()
        .unwrap()
        .iter()
        .map(|n| n.map(f32::to_bits))
        .collect();
    assert_eq!(hex(&bytes), OBLIQUE);
    assert_eq!(position_bits, OBLIQUE_POSITIONS);
    assert_eq!(normal_bits, OBLIQUE_NORMALS);
}

#[test]
fn quantized_mesh() {
    let mesh = triangle();
    let quantization = Quantization {
        position_error: Some(0.5),
        normal_angle: Some(0.25),
    };
    let bytes = mesh.encode(&quantization).unwrap();
    assert_eq!(hex(&bytes), QUANTIZED);
    let decoded = TriMesh::decode(&bytes, &Limits::STRICT).unwrap();
    assert_eq!(decoded.positions(), mesh.positions());
    assert_eq!(decoded.normals(), mesh.normals());
}

const EXACT: &str = "\
    4152434d01000003000000000000000000000000000000000000000000000000\
    00000000000000000000000000f03f0000000000000000000000000000000000\
    0000000000000000000000000000400000000000000000010000000000000000\
    0000000100000002000000050000000000000001000000000000000000000080\
    3f00000000000000000000803f00000000000000000000803f020000803f0000\
    000000000000000080bf0000803f0000000000000000000080bf0000803f0000\
    000000000000000080bf030100000000000000000000000000803f0000000000\
    0000000000803f040000803f0000003f0000803e0000803f0000803f0000003f\
    0000803e0000803f0000803f0000003f0000803e0000803f0501000000000000\
    007701000000000000003f0000803f0100000000000000000000000100000002\
    00000005000000010123456789ab7def8123456789abcdef";
const QUANTIZED: &str = "\
    4152434d01000101000000000000e03f00000000000000000000000000000000\
    0000000000000000000000000000f03f00000000000000400000000000000000\
    0300000000000000000000010000000100010000000000000000000000010000\
    00020000000500000000000000010102000000000000d03f0101010101010200\
    00803f0000000000000000000080bf0000803f0000000000000000000080bf00\
    00803f0000000000000000000080bf030100000000000000000000000000803f\
    00000000000000000000803f040000803f0000003f0000803e0000803f000080\
    3f0000003f0000803e0000803f0000803f0000003f0000803e0000803f050100\
    0000000000007701000000000000003f0000803f010000000000000000000000\
    010000000200000005000000010123456789ab7def8123456789abcdef";
const OBLIQUE: &str = "\
    4152434d010001087b14ae47e17a843f9a999999999901c0666666666666e6bf\
    666666666666febf9a99999999990d40cdcccccccccc08406666666666660240\
    04000000000000006300ffff4000009a7686ffb9020000000000000000000000\
    0100000002000000000000000200000003000000010000000000000001010afc\
    a9f1d24d62603f5402a9020000230354032e00240124010000000000000000";
/// Decoded independently from docs/architecture/mesh-encoding.md.
const OBLIQUE_POSITIONS: [[u64; 3]; 4] = [
    [
        0x3fb7_30ca_63fd_9740,
        0xbfe6_6666_6666_6666,
        0x4002_6666_6666_6665,
    ],
    [
        0x400d_9999_9999_999a,
        0x3fd0_3d09_d6a3_703c,
        0xbffe_6666_6666_6666,
    ],
    [
        0xc001_9999_9999_999a,
        0x3ff9_84b7_eb1e_5183,
        0x3fa6_497c_afe3_1640,
    ],
    [
        0x3fec_d003_3669_9cd0,
        0x4008_cccc_cccc_cccc,
        0x3ff2_5a5a_5a5a_5a58,
    ],
];
const OBLIQUE_NORMALS: [[u32; 3]; 4] = [
    [0x3e88_7e50, 0x3f08_7e50, 0x3f4d_8b04],
    [0xbf19_999a, 0x0000_0000, 0xbf4c_cccd],
    [0x3e09_19ea, 0xbefd_56d8, 0xbf5b_cf1d],
    [0xbf30_30f8, 0xbf30_30f8, 0x3e6a_ebf5],
];
