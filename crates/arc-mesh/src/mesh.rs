//! Indexed triangle meshes with attribute channels and face groups (CORE-MESH-001).

use std::collections::BTreeMap;

use arc_base::math::{Point3, Vec3f, vector3_to_gpu};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_geom::{Aabb3, TriangleMesh2};

use crate::error::codes;

/// The largest number of attribute channels a mesh holds.
pub const MAX_CHANNELS: usize = 64;

/// The key of an attribute channel. Each key fixes the channel's component type; channels are
/// kept, iterated and encoded in the order of their keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttributeKey {
    /// Vertex normals, [`Channel::Vec3`]. Directions; any finite length is accepted.
    Normal,
    /// Tangent frames, [`Channel::Vec4`]: the tangent direction in `x`, `y`, `z` and the
    /// handedness `w`, exactly 1 or −1. The bitangent is `w · (normal × tangent)`.
    Tangent,
    /// A texture coordinate set by its index, [`Channel::Vec2`].
    Uv(u32),
    /// Vertex colors as red, green, blue and alpha, [`Channel::Vec4`].
    Color,
    /// A named user channel of any component type. The name is not empty and at most
    /// `Limits::string_bytes` bytes long.
    User(String),
}

/// The values of an attribute channel, one per vertex, in `f32` (GPU precision).
#[derive(Debug, Clone, PartialEq)]
pub enum Channel {
    /// One component per vertex.
    Scalar(Vec<f32>),
    /// Two components per vertex.
    Vec2(Vec<[f32; 2]>),
    /// Three components per vertex.
    Vec3(Vec<[f32; 3]>),
    /// Four components per vertex.
    Vec4(Vec<[f32; 4]>),
}

impl Channel {
    /// The number of values (vertices).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Scalar(v) => v.len(),
            Self::Vec2(v) => v.len(),
            Self::Vec3(v) => v.len(),
            Self::Vec4(v) => v.len(),
        }
    }

    /// Whether the channel holds no values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of components per value, 1 to 4.
    #[must_use]
    pub fn components(&self) -> usize {
        match self {
            Self::Scalar(_) => 1,
            Self::Vec2(_) => 2,
            Self::Vec3(_) => 3,
            Self::Vec4(_) => 4,
        }
    }

    /// Every component, value by value.
    #[must_use]
    pub fn floats(&self) -> &[f32] {
        match self {
            Self::Scalar(v) => v,
            Self::Vec2(v) => v.as_flattened(),
            Self::Vec3(v) => v.as_flattened(),
            Self::Vec4(v) => v.as_flattened(),
        }
    }

    fn floats_mut(&mut self) -> &mut [f32] {
        match self {
            Self::Scalar(v) => v,
            Self::Vec2(v) => v.as_flattened_mut(),
            Self::Vec3(v) => v.as_flattened_mut(),
            Self::Vec4(v) => v.as_flattened_mut(),
        }
    }

    /// The channel with the value of vertex `map[i]` at position `i`. Indices must be in range;
    /// an index outside yields zeros instead of panicking.
    pub(crate) fn gather(&self, map: &[u32]) -> Self {
        fn pick<T: Copy + Default>(values: &[T], map: &[u32]) -> Vec<T> {
            map.iter()
                .map(|&i| values.get(i as usize).copied().unwrap_or_default())
                .collect()
        }
        match self {
            Self::Scalar(v) => Self::Scalar(pick(v, map)),
            Self::Vec2(v) => Self::Vec2(pick(v, map)),
            Self::Vec3(v) => Self::Vec3(pick(v, map)),
            Self::Vec4(v) => Self::Vec4(pick(v, map)),
        }
    }

    /// Appends the values of `other`; false when the component types differ.
    fn append(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Scalar(a), Self::Scalar(b)) => a.extend_from_slice(b),
            (Self::Vec2(a), Self::Vec2(b)) => a.extend_from_slice(b),
            (Self::Vec3(a), Self::Vec3(b)) => a.extend_from_slice(b),
            (Self::Vec4(a), Self::Vec4(b)) => a.extend_from_slice(b),
            _ => return false,
        }
        true
    }
}

/// The source of a face group: the object and its sub-object local key
/// (`(ObjectId, u32 local key)`, as the document model defines sub-object keys). `object` is
/// `None` when the mesh belongs to a single object that its holder already names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubObjectKey {
    /// The source object, if the mesh does not name it elsewhere.
    pub object: Option<ObjectId>,
    /// The local key assigned by the source object's type.
    pub local: u32,
}

/// A contiguous range of triangles `start..end` drawn with one material slot and picked as one
/// source sub-object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceGroup {
    /// The first triangle of the group.
    pub start: u32,
    /// One past the last triangle of the group.
    pub end: u32,
    /// The material slot, an index into the materials of the mesh's holder.
    pub material: u32,
    /// The source sub-object.
    pub key: SubObjectKey,
}

/// An indexed triangle mesh: `f64` model positions, `u32` triangles, attribute channels with
/// one value per vertex, and face groups.
///
/// Every mesh is validated on construction ([`TriMeshBuilder::build`]); the sign of zero is not
/// part of a value, so −0.0 is stored as +0.0 in positions and channels.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TriMesh {
    pub(crate) positions: Vec<Point3>,
    pub(crate) triangles: Vec<[u32; 3]>,
    pub(crate) channels: BTreeMap<AttributeKey, Channel>,
    pub(crate) groups: Vec<FaceGroup>,
}

/// Collects the parts of a [`TriMesh`] and validates them in [`Self::build`].
#[derive(Debug, Clone, Default)]
pub struct TriMeshBuilder {
    positions: Vec<Point3>,
    triangles: Vec<[u32; 3]>,
    channels: BTreeMap<AttributeKey, Channel>,
    groups: Vec<FaceGroup>,
}

impl TriMeshBuilder {
    /// Starts a mesh from positions and triangles of vertex indices.
    #[must_use]
    pub fn new(positions: Vec<Point3>, triangles: Vec<[u32; 3]>) -> Self {
        Self {
            positions,
            triangles,
            ..Self::default()
        }
    }

    /// Sets the channel under `key`, replacing a channel with the same key.
    #[must_use]
    pub fn channel(mut self, key: AttributeKey, channel: Channel) -> Self {
        self.channels.insert(key, channel);
        self
    }

    /// Removes the channel under `key`.
    #[must_use]
    pub fn without_channel(mut self, key: &AttributeKey) -> Self {
        self.channels.remove(key);
        self
    }

    /// Appends a face group; groups must be given in triangle order.
    #[must_use]
    pub fn group(mut self, group: FaceGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// Validates the parts and builds the mesh.
    ///
    /// # Errors
    ///
    /// In this order: `mesh.limit.exceeded` when the vertex or triangle count exceeds
    /// `u32::MAX` or `limits.array_length`; `mesh.channel.invalid` for more than
    /// [`MAX_CHANNELS`] channels; `mesh.value.not_finite` for a non-finite position;
    /// `mesh.index.out_of_range` for a triangle index not below the vertex count; per channel in
    /// key order, `mesh.channel.invalid` for a component type that does not match the key or an
    /// empty or oversized user name, `mesh.channel.length_mismatch` for a length other than the
    /// vertex count, `mesh.value.not_finite` for a non-finite component and
    /// `mesh.channel.invalid` for a tangent handedness other than ±1; `mesh.group.invalid`
    /// for a group that is empty, ends past the last triangle or starts before the previous
    /// group ends.
    pub fn build(mut self, limits: &Limits) -> Result<TriMesh> {
        let fits = |count: usize| {
            u32::try_from(count).is_ok()
                && u64::try_from(count).is_ok_and(|count| count <= limits.array_length)
        };
        if !fits(self.positions.len()) || !fits(self.triangles.len()) {
            return Err(Error::new(codes::LIMIT_EXCEEDED));
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(Error::new(codes::CHANNEL_INVALID));
        }
        if !self
            .positions
            .iter()
            .all(|p| p.iter().all(|c| c.is_finite()))
        {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
        let vertices = self.positions.len();
        if let Some(triangle) = self
            .triangles
            .iter()
            .position(|t| t.iter().any(|&i| i as usize >= vertices))
        {
            return Err(Error::new(codes::INDEX_OUT_OF_RANGE)
                .with_detail("triangle", i64::try_from(triangle).unwrap_or(i64::MAX)));
        }
        for (key, channel) in &self.channels {
            check_channel(key, channel, vertices, limits)?;
        }
        let mut previous_end = 0;
        for group in &self.groups {
            if group.start >= group.end
                || group.start < previous_end
                || group.end as usize > self.triangles.len()
            {
                return Err(Error::new(codes::GROUP_INVALID));
            }
            previous_end = group.end;
        }
        // −0.0 + 0.0 is +0.0; every other value is unchanged.
        for p in &mut self.positions {
            p.iter_mut().for_each(|c| *c += 0.0);
        }
        for channel in self.channels.values_mut() {
            channel.floats_mut().iter_mut().for_each(|c| *c += 0.0);
        }
        Ok(TriMesh {
            positions: self.positions,
            triangles: self.triangles,
            channels: self.channels,
            groups: self.groups,
        })
    }
}

fn check_channel(
    key: &AttributeKey,
    channel: &Channel,
    vertices: usize,
    limits: &Limits,
) -> Result<()> {
    let typed = match key {
        AttributeKey::Normal => matches!(channel, Channel::Vec3(_)),
        AttributeKey::Tangent | AttributeKey::Color => matches!(channel, Channel::Vec4(_)),
        AttributeKey::Uv(_) => matches!(channel, Channel::Vec2(_)),
        AttributeKey::User(name) => {
            !name.is_empty() && u64::try_from(name.len()).is_ok_and(|n| n <= limits.string_bytes)
        }
    };
    if !typed {
        return Err(Error::new(codes::CHANNEL_INVALID));
    }
    if channel.len() != vertices {
        return Err(Error::new(codes::CHANNEL_LENGTH_MISMATCH));
    }
    if !channel.floats().iter().all(|c| c.is_finite()) {
        return Err(Error::new(codes::VALUE_NOT_FINITE));
    }
    if let (AttributeKey::Tangent, Channel::Vec4(tangents)) = (key, channel)
        && !tangents.iter().all(|t| t[3] == 1.0 || t[3] == -1.0)
    {
        return Err(Error::new(codes::CHANNEL_INVALID));
    }
    Ok(())
}

impl TriMesh {
    /// A mesh of positions and triangles without channels or groups.
    ///
    /// # Errors
    ///
    /// As [`TriMeshBuilder::build`].
    pub fn new(positions: Vec<Point3>, triangles: Vec<[u32; 3]>, limits: &Limits) -> Result<Self> {
        TriMeshBuilder::new(positions, triangles).build(limits)
    }

    /// The 2D triangle mesh placed in the plane `z = elevation`. Counterclockwise triangles face
    /// +Z. Place it elsewhere with [`Self::transformed`].
    ///
    /// # Errors
    ///
    /// `mesh.value.not_finite` when `elevation` is not finite; otherwise as
    /// [`TriMeshBuilder::build`].
    pub fn from_triangle_mesh2(
        mesh: &TriangleMesh2,
        elevation: f64,
        limits: &Limits,
    ) -> Result<Self> {
        let positions = mesh
            .vertices()
            .iter()
            .map(|p| Point3::new(p.x, p.y, elevation))
            .collect();
        Self::new(positions, mesh.triangles().to_vec(), limits)
    }

    /// The parts of this mesh in a builder, to change and validate again.
    #[must_use]
    pub fn into_builder(self) -> TriMeshBuilder {
        TriMeshBuilder {
            positions: self.positions,
            triangles: self.triangles,
            channels: self.channels,
            groups: self.groups,
        }
    }

    /// The vertex positions in model coordinates.
    #[must_use]
    pub fn positions(&self) -> &[Point3] {
        &self.positions
    }

    /// The triangles, three vertex indices each.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// The number of vertices.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// The number of triangles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// The channels in key order.
    #[must_use]
    pub fn channels(&self) -> &BTreeMap<AttributeKey, Channel> {
        &self.channels
    }

    /// The channel under `key`.
    #[must_use]
    pub fn channel(&self, key: &AttributeKey) -> Option<&Channel> {
        self.channels.get(key)
    }

    /// The vertex normals.
    #[must_use]
    pub fn normals(&self) -> Option<&[[f32; 3]]> {
        match self.channels.get(&AttributeKey::Normal) {
            Some(Channel::Vec3(v)) => Some(v),
            _ => None,
        }
    }

    /// The tangent frames: direction and handedness.
    #[must_use]
    pub fn tangents(&self) -> Option<&[[f32; 4]]> {
        match self.channels.get(&AttributeKey::Tangent) {
            Some(Channel::Vec4(v)) => Some(v),
            _ => None,
        }
    }

    /// The texture coordinates of set `set`.
    #[must_use]
    pub fn uv_set(&self, set: u32) -> Option<&[[f32; 2]]> {
        match self.channels.get(&AttributeKey::Uv(set)) {
            Some(Channel::Vec2(v)) => Some(v),
            _ => None,
        }
    }

    /// The vertex colors.
    #[must_use]
    pub fn colors(&self) -> Option<&[[f32; 4]]> {
        match self.channels.get(&AttributeKey::Color) {
            Some(Channel::Vec4(v)) => Some(v),
            _ => None,
        }
    }

    /// The face groups in triangle order.
    #[must_use]
    pub fn groups(&self) -> &[FaceGroup] {
        &self.groups
    }

    /// The group that contains `triangle`, if any; triangles between groups belong to none.
    #[must_use]
    pub fn group_of(&self, triangle: u32) -> Option<&FaceGroup> {
        let after = self.groups.partition_point(|g| g.start <= triangle);
        after
            .checked_sub(1)
            .and_then(|i| self.groups.get(i))
            .filter(|g| triangle < g.end)
    }

    /// The bounding box of the positions; empty for a mesh without vertices.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb3 {
        Aabb3::from_points(&self.positions)
    }

    /// The positions relative to `origin`, narrowed to `f32` for the GPU: the difference is
    /// taken in `f64`, then narrowed, so precision is highest near `origin`.
    ///
    /// # Errors
    ///
    /// `mesh.value.not_finite` when `origin` is not finite; `mesh.precision.insufficient` when
    /// a relative position exceeds the range of `f32`.
    pub fn gpu_positions(&self, origin: &Point3) -> Result<Vec<Vec3f>> {
        if !origin.iter().all(|c| c.is_finite()) {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
        self.positions
            .iter()
            .map(|p| {
                let relative = vector3_to_gpu(&(p - origin));
                if relative.is_finite() {
                    Ok(relative)
                } else {
                    Err(Error::new(codes::PRECISION_INSUFFICIENT))
                }
            })
            .collect()
    }

    /// The indices of the degenerate triangles, in order: a triangle is degenerate when two of
    /// its indices are equal or its smallest height (twice its area over its longest edge) is
    /// at most `tolerance.linear`. Degenerate triangles are reported, not removed.
    ///
    /// # Errors
    ///
    /// `mesh.tolerance.invalid` when `tolerance.linear` is negative or not finite.
    pub fn degenerate_triangles(&self, tolerance: &Tolerance) -> Result<Vec<u32>> {
        crate::ensure_tolerance(tolerance)?;
        Ok((0_u32..)
            .zip(&self.triangles)
            .filter(|(_, t)| crate::normals::face(&self.positions, **t, tolerance.linear).is_none())
            .map(|(i, _)| i)
            .collect())
    }

    /// The meshes concatenated in order: indices and groups are offset, channels appended.
    /// No meshes give the empty mesh.
    ///
    /// # Errors
    ///
    /// `mesh.merge.incompatible` when the meshes do not have the same channel keys with the
    /// same component types; `mesh.limit.exceeded` when the result exceeds `u32` indices or
    /// `limits`.
    pub fn merge<'a>(
        meshes: impl IntoIterator<Item = &'a TriMesh>,
        limits: &Limits,
    ) -> Result<TriMesh> {
        let mut meshes = meshes.into_iter();
        let Some(first) = meshes.next() else {
            return Ok(TriMesh::default());
        };
        let mut merged = first.clone();
        for mesh in meshes {
            let vertex_offset = u32::try_from(merged.positions.len())
                .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
            let triangle_offset = u32::try_from(merged.triangles.len())
                .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
            if !mesh.channels.keys().eq(merged.channels.keys()) {
                return Err(Error::new(codes::MERGE_INCOMPATIBLE));
            }
            for (channel, other) in merged.channels.values_mut().zip(mesh.channels.values()) {
                if !channel.append(other) {
                    return Err(Error::new(codes::MERGE_INCOMPATIBLE));
                }
            }
            merged.positions.extend_from_slice(&mesh.positions);
            for t in &mesh.triangles {
                let shifted = t.map(|i| i.checked_add(vertex_offset));
                let [Some(a), Some(b), Some(c)] = shifted else {
                    return Err(Error::new(codes::LIMIT_EXCEEDED));
                };
                merged.triangles.push([a, b, c]);
            }
            for g in &mesh.groups {
                let (Some(start), Some(end)) = (
                    g.start.checked_add(triangle_offset),
                    g.end.checked_add(triangle_offset),
                ) else {
                    return Err(Error::new(codes::LIMIT_EXCEEDED));
                };
                merged.groups.push(FaceGroup { start, end, ..*g });
            }
        }
        merged.into_builder().build(limits)
    }

    /// A builder with new positions and triangles, every channel gathered through `remap`
    /// (output vertex to vertex of this mesh) and the groups of this mesh.
    pub(crate) fn rebuilt(
        &self,
        positions: Vec<Point3>,
        triangles: Vec<[u32; 3]>,
        remap: &[u32],
    ) -> TriMeshBuilder {
        let mut builder = TriMeshBuilder::new(positions, triangles);
        for (key, channel) in &self.channels {
            builder = builder.channel(key.clone(), channel.gather(remap));
        }
        builder.groups.clone_from(&self.groups);
        builder
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test values are small and exact enough"
)]
mod tests {
    use arc_base::math::Point2;

    use super::*;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    fn quad() -> TriMeshBuilder {
        TriMeshBuilder::new(
            vec![
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(1.0, 1.0, 0.0),
                p(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    fn group(start: u32, end: u32) -> FaceGroup {
        FaceGroup {
            start,
            end,
            material: 7,
            key: SubObjectKey {
                object: None,
                local: start,
            },
        }
    }

    fn code(result: Result<TriMesh>) -> &'static str {
        result.unwrap_err().code()
    }

    #[test]
    fn construction_validates_every_part() {
        let limits = Limits::STRICT;
        assert!(quad().build(&limits).is_ok());
        assert_eq!(
            code(TriMesh::new(
                vec![p(0.0, 0.0, 0.0)],
                vec![[0, 0, 1]],
                &limits
            )),
            "mesh.index.out_of_range"
        );
        assert_eq!(
            code(TriMesh::new(vec![p(f64::NAN, 0.0, 0.0)], vec![], &limits)),
            "mesh.value.not_finite"
        );
        let short = quad().channel(AttributeKey::Normal, Channel::Vec3(vec![[0.0; 3]; 3]));
        assert_eq!(code(short.build(&limits)), "mesh.channel.length_mismatch");
        let wrong_type = quad().channel(AttributeKey::Uv(0), Channel::Vec3(vec![[0.0; 3]; 4]));
        assert_eq!(code(wrong_type.build(&limits)), "mesh.channel.invalid");
        let handed = quad().channel(
            AttributeKey::Tangent,
            Channel::Vec4(vec![[1.0, 0.0, 0.0, 0.5]; 4]),
        );
        assert_eq!(code(handed.build(&limits)), "mesh.channel.invalid");
        let unnamed = quad().channel(
            AttributeKey::User(String::new()),
            Channel::Scalar(vec![0.0; 4]),
        );
        assert_eq!(code(unnamed.build(&limits)), "mesh.channel.invalid");
        let infinite = quad().channel(
            AttributeKey::Color,
            Channel::Vec4(vec![[f32::INFINITY; 4]; 4]),
        );
        assert_eq!(code(infinite.build(&limits)), "mesh.value.not_finite");
        let mut many = quad();
        for i in 0..=MAX_CHANNELS as u32 {
            many = many.channel(AttributeKey::Uv(i), Channel::Vec2(vec![[0.0; 2]; 4]));
        }
        assert_eq!(code(many.build(&limits)), "mesh.channel.invalid");
        let tight = Limits {
            array_length: 3,
            ..limits
        };
        assert_eq!(code(quad().build(&tight)), "mesh.limit.exceeded");
    }

    #[test]
    fn groups_must_be_ordered_disjoint_nonempty_and_in_range() {
        let limits = Limits::STRICT;
        let ok = quad()
            .group(group(0, 1))
            .group(group(1, 2))
            .build(&limits)
            .unwrap();
        assert_eq!(ok.group_of(1).unwrap().start, 1);
        let gap = quad().group(group(1, 2)).build(&limits).unwrap();
        assert_eq!(gap.group_of(0), None);
        assert_eq!(gap.group_of(1), Some(&group(1, 2)));
        assert_eq!(gap.group_of(2), None);
        for bad in [
            vec![group(1, 1)],
            vec![group(0, 3)],
            vec![group(0, 2), group(1, 2)],
            vec![group(1, 2), group(0, 1)],
        ] {
            let mut builder = quad();
            for g in bad {
                builder = builder.group(g);
            }
            assert_eq!(code(builder.build(&limits)), "mesh.group.invalid");
        }
    }

    #[test]
    fn negative_zero_is_stored_as_positive_zero() {
        let mesh = TriMeshBuilder::new(vec![p(-0.0, 1.0, -0.0)], vec![])
            .channel(AttributeKey::User("w".into()), Channel::Scalar(vec![-0.0]))
            .build(&Limits::STRICT)
            .unwrap();
        assert!(mesh.positions()[0].x.is_sign_positive());
        assert!(
            mesh.channel(&AttributeKey::User("w".into()))
                .unwrap()
                .floats()[0]
                .is_sign_positive()
        );
    }

    #[test]
    fn bounding_box_and_gpu_positions_are_relative_to_the_origin() {
        let mesh = TriMesh::new(
            vec![p(1e9 + 0.25, 2.0, 3.0), p(1e9 + 1.5, -1.0, 0.0)],
            vec![],
            &Limits::STRICT,
        )
        .unwrap();
        let bbox = mesh.bounding_box();
        assert_eq!(bbox.min(), p(1e9 + 0.25, -1.0, 0.0));
        let gpu = mesh.gpu_positions(&p(1e9, 0.0, 0.0)).unwrap();
        assert_eq!(gpu[0], Vec3f::new(0.25, 2.0, 3.0));
        assert_eq!(gpu[1], Vec3f::new(1.5, -1.0, 0.0));
        let far = TriMesh::new(vec![p(1e300, 0.0, 0.0)], vec![], &Limits::STRICT).unwrap();
        assert_eq!(
            far.gpu_positions(&Point3::origin()).unwrap_err().code(),
            "mesh.precision.insufficient"
        );
    }

    #[test]
    fn degenerate_triangles_are_reported_not_removed() {
        let mesh = TriMesh::new(
            vec![
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(0.0, 1.0, 0.0),
                p(2.0, 0.0, 0.0),
                p(0.5, 1e-9, 0.0),
            ],
            vec![[0, 1, 2], [0, 0, 1], [0, 1, 3], [0, 3, 4], [0, 1, 2]],
            &Limits::STRICT,
        )
        .unwrap();
        assert_eq!(
            mesh.degenerate_triangles(&Tolerance::DEFAULT).unwrap(),
            vec![1, 2, 3]
        );
        let exact = Tolerance {
            linear: 0.0,
            ..Tolerance::DEFAULT
        };
        assert_eq!(mesh.degenerate_triangles(&exact).unwrap(), vec![1, 2]);
        let bad = Tolerance {
            linear: -1.0,
            ..Tolerance::DEFAULT
        };
        assert_eq!(
            mesh.degenerate_triangles(&bad).unwrap_err().code(),
            "mesh.tolerance.invalid"
        );
        assert_eq!(mesh.triangle_count(), 5);
    }

    #[test]
    fn merging_offsets_indices_and_groups_and_needs_equal_channels() {
        let limits = Limits::STRICT;
        let a = quad().group(group(0, 2)).build(&limits).unwrap();
        let merged = TriMesh::merge([&a, &a], &limits).unwrap();
        assert_eq!(merged.vertex_count(), 8);
        assert_eq!(merged.triangles()[3], [4, 6, 7]);
        assert_eq!(merged.groups()[1].start, 2);
        assert_eq!(merged.groups()[1].end, 4);
        assert_eq!(TriMesh::merge([], &limits).unwrap(), TriMesh::default());
        let colored = quad()
            .channel(AttributeKey::Color, Channel::Vec4(vec![[1.0; 4]; 4]))
            .build(&limits)
            .unwrap();
        assert_eq!(
            TriMesh::merge([&a, &colored], &limits).unwrap_err().code(),
            "mesh.merge.incompatible"
        );
        let scalar = quad()
            .channel(
                AttributeKey::User("u".into()),
                Channel::Scalar(vec![1.0; 4]),
            )
            .build(&limits)
            .unwrap();
        let vector = quad()
            .channel(
                AttributeKey::User("u".into()),
                Channel::Vec2(vec![[1.0; 2]; 4]),
            )
            .build(&limits)
            .unwrap();
        assert_eq!(
            TriMesh::merge([&scalar, &vector], &limits)
                .unwrap_err()
                .code(),
            "mesh.merge.incompatible"
        );
        let tight = Limits {
            array_length: 6,
            ..limits
        };
        assert_eq!(
            TriMesh::merge([&a, &a], &tight).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
    }

    #[test]
    fn a_2d_triangle_mesh_is_placed_at_an_elevation() {
        let flat = TriangleMesh2::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 0.0),
                Point2::new(0.0, 1.0),
            ],
            vec![[0, 1, 2]],
        )
        .unwrap();
        let mesh = TriMesh::from_triangle_mesh2(&flat, 2.5, &Limits::STRICT).unwrap();
        assert_eq!(mesh.positions()[1], p(1.0, 0.0, 2.5));
        assert_eq!(mesh.triangles(), &[[0, 1, 2]]);
        assert_eq!(
            TriMesh::from_triangle_mesh2(&flat, f64::NAN, &Limits::STRICT)
                .unwrap_err()
                .code(),
            "mesh.value.not_finite"
        );
    }
}
