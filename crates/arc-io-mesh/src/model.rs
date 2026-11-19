//! The neutral mesh model (CORE-IO-001): named triangle meshes with an optional material name,
//! and the materials of an OBJ material library.
//!
//! Positions are the triangle mesh's `f64` positions. Per-vertex normals, texture coordinates and
//! colors are its attribute channels (`AttributeKey::Normal`, `Uv(0)` and `Color`), so no format
//! carries a second copy of them. The model is deliberately small; it is the base that a glTF
//! reader extends later.

use arc_base::{Limits, Result};
use arc_mesh::{AttributeKey, Channel, TriMesh, TriMeshBuilder};

/// A scene of triangle meshes and the materials they refer to by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshScene {
    /// The meshes, in file order.
    pub meshes: Vec<MeshPart>,
    /// The materials of the material library, in file order.
    pub materials: Vec<Material>,
}

/// A named triangle mesh: an OBJ object or group, an STL solid, or the one mesh of a PLY file.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshPart {
    /// The name; empty when the format gives none.
    pub name: String,
    /// The triangles, positions and attribute channels.
    pub mesh: TriMesh,
    /// The name of the material the part is drawn with, if any.
    pub material: Option<String>,
}

/// A material of an MTL library. Colors are linear values as written in the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Material {
    /// The name the faces refer to.
    pub name: String,
    /// The diffuse color (`Kd`).
    pub diffuse: Option<[f32; 3]>,
    /// The ambient color (`Ka`).
    pub ambient: Option<[f32; 3]>,
    /// The specular color (`Ks`).
    pub specular: Option<[f32; 3]>,
    /// The opacity, 1 for opaque (`d`, or `1 - Tr`).
    pub opacity: Option<f32>,
    /// The texture maps, in file order.
    pub textures: Vec<TextureMap>,
}

/// A texture statement of a material, such as `map_Kd`. The text after the statement name is kept
/// as written: options and the file name, which this crate never opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureMap {
    /// The statement name, such as `map_Kd` or `bump`.
    pub slot: String,
    /// The text after the statement name.
    pub value: String,
}

/// The name of an attribute channel as the loss reports name it.
pub(crate) fn attribute_name(key: &AttributeKey) -> &'static str {
    match key {
        AttributeKey::Normal => "normal",
        AttributeKey::Tangent => "tangent",
        AttributeKey::Uv(0) => "uv",
        AttributeKey::Uv(_) => "uv set",
        AttributeKey::Color => "color",
        AttributeKey::User(_) => "user channel",
    }
}

/// Builds a validated triangle mesh from the parts a reader collected.
pub(crate) fn build_mesh(
    positions: Vec<arc_base::math::Point3>,
    triangles: Vec<[u32; 3]>,
    channels: Vec<(AttributeKey, Channel)>,
    limits: &Limits,
) -> Result<TriMesh> {
    let mut builder = TriMeshBuilder::new(positions, triangles);
    for (key, channel) in channels {
        builder = builder.channel(key, channel);
    }
    builder.build(limits)
}
