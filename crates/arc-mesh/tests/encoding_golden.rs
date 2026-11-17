//! Golden vectors of the mesh encoding (CORE-MESH-002): the exact bytes of one exact and one
//! quantized mesh, so that any change of the byte layout fails here. Changing a vector means a
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
    4152434d01000102000000000000e03f00000000000000000000000000000000\
    0000000000000000000000000000f03f00000000000000400000000000000000\
    0300000000000000000000030000000300010000000000000000000000010000\
    00020000000500000000000000010102000000000000d03f0101010101010200\
    00803f0000000000000000000080bf0000803f0000000000000000000080bf00\
    00803f0000000000000000000080bf030100000000000000000000000000803f\
    00000000000000000000803f040000803f0000003f0000803e0000803f000080\
    3f0000003f0000803e0000803f0000803f0000003f0000803e0000803f050100\
    0000000000007701000000000000003f0000803f010000000000000000000000\
    010000000200000005000000010123456789ab7def8123456789abcdef";
