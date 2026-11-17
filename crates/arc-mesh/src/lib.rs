//! Polygon meshes of the Arc suite.
//!
//! This first part provides indexed triangle meshes with attribute channels and face groups
//! (CORE-MESH-001), their versioned binary encoding with optional quantization
//! (CORE-MESH-002), and flat, area-weighted and angle-weighted normals with a crease angle and
//! tangent frames (CORE-MESH-003).
//!
//! Positions are `f64` model coordinates; attribute values are `f32`, the precision the GPU
//! uses. Every [`TriMesh`] is validated when it is built or decoded, so an index outside the
//! vertices, a channel of the wrong length or a non-finite value cannot exist. Comparisons that
//! depend on a tolerance take an [`arc_base::Tolerance`].
//!
//! No function in this crate panics.

pub mod encoding;
pub mod error;
pub mod mesh;
pub mod normals;
pub mod transform;

pub use encoding::{MeshEncoding, Quantization};
pub use mesh::{
    AttributeKey, Channel, FaceGroup, MAX_CHANNELS, SubObjectKey, TriMesh, TriMeshBuilder,
};
pub use normals::{FALLBACK_NORMAL, NormalWeighting};

use arc_base::math::Vector3;
use arc_base::{Error, Result, Tolerance};

/// Fails with `mesh.tolerance.invalid` unless the linear tolerance is finite and not negative.
pub(crate) fn ensure_tolerance(tolerance: &Tolerance) -> Result<()> {
    // Written so that NaN fails the check.
    if tolerance.linear.is_finite() && tolerance.linear >= 0.0 {
        Ok(())
    } else {
        Err(Error::new(error::codes::TOLERANCE_INVALID))
    }
}

/// `v` scaled to unit length without overflow or underflow of intermediate squares, or `None`
/// for the zero vector and for non-finite input.
pub(crate) fn unit(v: &Vector3) -> Option<Vector3> {
    let largest = v.amax();
    if !(largest > 0.0 && largest.is_finite()) {
        return None;
    }
    let scaled = v / largest;
    Some(scaled / scaled.norm())
}

/// The angle between two directions in [0, π], accurate for small and large angles alike.
pub(crate) fn angle_between(a: &Vector3, b: &Vector3) -> f64 {
    a.cross(b).norm().atan2(a.dot(b))
}

/// The Euclidean length of `v` without overflow or underflow of intermediate squares.
pub(crate) fn length(v: &Vector3) -> f64 {
    let largest = v.amax();
    if largest > 0.0 && largest.is_finite() {
        (v / largest).norm() * largest
    } else {
        largest
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test values are small and exact enough"
)]
pub(crate) mod testing {
    //! Builders and strategies shared by the tests of this crate.

    use arc_base::Limits;
    use arc_base::math::Point3;
    use proptest::prelude::*;

    use crate::{AttributeKey, Channel, TriMesh, TriMeshBuilder};

    /// The unit cube as 8 shared vertices and 12 outward-facing triangles.
    pub fn cube() -> TriMesh {
        let positions = (0..8)
            .map(|i| Point3::new(f64::from(i & 1), f64::from((i >> 1) & 1), f64::from(i >> 2)))
            .collect();
        let triangles = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        TriMesh::new(positions, triangles, &Limits::STRICT).unwrap()
    }

    /// A UV sphere of radius `radius` around `center`, with `rings` latitude bands and
    /// `segments` longitude bands, poles as single vertices and UVs.
    pub fn sphere(center: Point3, radius: f64, rings: u32, segments: u32) -> TriMesh {
        let mut positions = vec![center + arc_base::math::Vector3::z() * radius];
        let mut uvs = vec![[0.5, 0.0]];
        for ring in 1..rings {
            let polar = std::f64::consts::PI * f64::from(ring) / f64::from(rings);
            for segment in 0..segments {
                let azimuth = std::f64::consts::TAU * f64::from(segment) / f64::from(segments);
                let direction = arc_base::math::Vector3::new(
                    polar.sin() * azimuth.cos(),
                    polar.sin() * azimuth.sin(),
                    polar.cos(),
                );
                positions.push(center + direction * radius);
                uvs.push([segment as f32 / segments as f32, ring as f32 / rings as f32]);
            }
        }
        positions.push(center - arc_base::math::Vector3::z() * radius);
        uvs.push([0.5, 1.0]);
        let south = rings.saturating_sub(1) * segments + 1;
        let at = |ring: u32, segment: u32| 1 + (ring - 1) * segments + segment % segments;
        let mut triangles = Vec::new();
        for s in 0..segments {
            triangles.push([0, at(1, s), at(1, s + 1)]);
            for ring in 1..rings - 1 {
                let (a, b) = (at(ring, s), at(ring, s + 1));
                let (c, d) = (at(ring + 1, s), at(ring + 1, s + 1));
                triangles.push([a, c, d]);
                triangles.push([a, d, b]);
            }
            triangles.push([at(rings - 1, s), south, at(rings - 1, s + 1)]);
        }
        TriMeshBuilder::new(positions, triangles)
            .channel(AttributeKey::Uv(0), Channel::Vec2(uvs))
            .build(&Limits::DEFAULT)
            .unwrap()
    }

    /// A finite `f64` coordinate: ordinary, or of extreme magnitude, or zero.
    pub fn coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![
            4 => -100.0..100.0_f64,
            1 => (-150.0..150.0_f64, any::<bool>())
                .prop_map(|(e, negative)| if negative { -(10.0_f64.powf(e)) } else { 10.0_f64.powf(e) }),
            1 => Just(0.0),
        ]
    }

    /// A finite `f32` component of either sign, including zero.
    pub fn component() -> impl Strategy<Value = f32> {
        prop_oneof![
            4 => -10.0..10.0_f32,
            1 => (-30.0..30.0_f32, any::<bool>())
                .prop_map(|(e, negative)| if negative { -(10.0_f32.powf(e)) } else { 10.0_f32.powf(e) }),
            1 => Just(0.0_f32),
        ]
    }

    /// A random valid mesh with every kind of channel and some groups.
    pub fn mesh() -> impl Strategy<Value = TriMesh> {
        (1_usize..24).prop_flat_map(|vertices| {
            let n = u32::try_from(vertices).unwrap();
            (
                prop::collection::vec((coordinate(), coordinate(), coordinate()), vertices),
                prop::collection::vec([0..n, 0..n, 0..n], 0..30),
                prop::collection::vec([component(), component(), component()], vertices),
                prop::collection::vec(
                    ([component(), component(), component()], any::<bool>()),
                    vertices,
                ),
                prop::collection::vec([component(), component()], vertices),
                prop::collection::vec(component(), vertices),
                any::<u64>(),
            )
                .prop_map(
                    |(positions, triangles, normals, tangents, uvs, user, seed)| {
                        let positions = positions
                            .into_iter()
                            .map(|(x, y, z)| Point3::new(x, y, z))
                            .collect();
                        let count = u32::try_from(triangles.len()).unwrap();
                        let tangents = tangents
                            .into_iter()
                            .map(|([x, y, z], left)| [x, y, z, if left { -1.0 } else { 1.0 }])
                            .collect();
                        let mut builder = TriMeshBuilder::new(positions, triangles)
                            .channel(AttributeKey::Normal, Channel::Vec3(normals))
                            .channel(AttributeKey::Tangent, Channel::Vec4(tangents))
                            .channel(AttributeKey::Uv(3), Channel::Vec2(uvs))
                            .channel(AttributeKey::User("weight".into()), Channel::Scalar(user));
                        if count > 1 {
                            let split = u32::try_from(seed % u64::from(count)).unwrap().max(1);
                            let object = (seed % 3 == 0).then(|| {
                                <arc_base::ObjectId as arc_base::id::Identifier>::from_u128(
                                    u128::from(seed) << 64 | 0x1234,
                                )
                            });
                            builder = builder
                                .group(crate::FaceGroup {
                                    start: 0,
                                    end: split,
                                    material: 1,
                                    key: crate::SubObjectKey { object, local: 9 },
                                })
                                .group(crate::FaceGroup {
                                    start: split,
                                    end: count,
                                    material: u32::MAX,
                                    key: crate::SubObjectKey {
                                        object: None,
                                        local: u32::try_from(seed >> 40).unwrap(),
                                    },
                                });
                        }
                        builder.build(&Limits::STRICT).unwrap()
                    },
                )
        })
    }
}
