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
///
/// OBJ, STL and PLY use `meshes` and `materials` only. A glTF file also fills the scene hierarchy:
/// `mesh_groups` are its meshes (each a list of parts, one per primitive), `nodes` are its nodes in
/// file order, `roots` are the nodes of the default scene, and `cameras` and `lights` are its
/// cameras and `KHR_lights_punctual` lights. A node names a mesh group by index, so a mesh that
/// several nodes share is stored once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshScene {
    /// The meshes, in file order.
    pub meshes: Vec<MeshPart>,
    /// The materials of the material library, in file order.
    pub materials: Vec<Material>,
    /// The meshes of a glTF file, in file order; each names parts of `meshes` by index.
    pub mesh_groups: Vec<MeshGroup>,
    /// The nodes of the scene hierarchy, in file order.
    pub nodes: Vec<Node>,
    /// The indices of the root nodes of the default scene.
    pub roots: Vec<usize>,
    /// The cameras, in file order.
    pub cameras: Vec<Camera>,
    /// The `KHR_lights_punctual` lights, in file order.
    pub lights: Vec<Light>,
    /// Unsupported extensions and extras of the document itself, kept as JSON text.
    pub preserved: Preserved,
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
    /// Unsupported extensions and extras of the glTF primitive the part came from.
    pub preserved: Preserved,
}

/// A glTF mesh: the parts (primitives) it is made of, by index into `MeshScene::meshes`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshGroup {
    /// The name; empty when the file gives none.
    pub name: String,
    /// Indices into `MeshScene::meshes`, in the order of the file.
    pub primitives: Vec<usize>,
    /// Unsupported extensions and extras of the mesh.
    pub preserved: Preserved,
}

/// The transform of a node: a matrix, or translation, rotation and scale. Components the file
/// does not give hold the glTF defaults (zero, the identity quaternion and one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    /// A column-major 4 by 4 matrix.
    Matrix([f32; 16]),
    /// Translation, a unit quaternion `[x, y, z, w]` and scale.
    Trs {
        /// The translation.
        translation: [f32; 3],
        /// The rotation as a quaternion `[x, y, z, w]`.
        rotation: [f32; 4],
        /// The scale.
        scale: [f32; 3],
    },
}

impl Default for Transform {
    fn default() -> Self {
        Self::Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// A node of the scene hierarchy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    /// The name; empty when the file gives none.
    pub name: String,
    /// The transform relative to the parent.
    pub transform: Transform,
    /// The mesh group the node draws, by index into `MeshScene::mesh_groups`.
    pub mesh: Option<usize>,
    /// The child nodes, by index into `MeshScene::nodes`.
    pub children: Vec<usize>,
    /// The camera, by index into `MeshScene::cameras`.
    pub camera: Option<usize>,
    /// The light, by index into `MeshScene::lights`.
    pub light: Option<usize>,
    /// Unsupported extensions and extras of the node.
    pub preserved: Preserved,
}

/// The projection of a camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// A perspective projection.
    Perspective {
        /// The vertical field of view in radians.
        yfov: f32,
        /// The aspect ratio, if the file gives one.
        aspect_ratio: Option<f32>,
        /// The near clipping distance.
        znear: f32,
        /// The far clipping distance, if the file gives one.
        zfar: Option<f32>,
    },
    /// An orthographic projection.
    Orthographic {
        /// The horizontal magnification.
        xmag: f32,
        /// The vertical magnification.
        ymag: f32,
        /// The near clipping distance.
        znear: f32,
        /// The far clipping distance.
        zfar: f32,
    },
}

/// A camera.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// The name; empty when the file gives none.
    pub name: String,
    /// The projection.
    pub projection: Projection,
    /// Unsupported extensions and extras of the camera.
    pub preserved: Preserved,
}

/// The kind of a `KHR_lights_punctual` light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightKind {
    /// A light at infinity, along the negative z axis of its node.
    Directional,
    /// A light at a point, in every direction.
    Point,
    /// A light at a point, within a cone.
    Spot {
        /// The angle in radians at which the falloff starts.
        inner_cone_angle: f32,
        /// The angle in radians at which the light ends.
        outer_cone_angle: f32,
    },
}

/// A `KHR_lights_punctual` light. Missing color and intensity hold the extension's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Light {
    /// The name; empty when the file gives none.
    pub name: String,
    /// The kind of light.
    pub kind: LightKind,
    /// The color, linear RGB.
    pub color: [f32; 3],
    /// The intensity.
    pub intensity: f32,
    /// The range beyond which the light has no effect, if the file gives one.
    pub range: Option<f32>,
    /// Unsupported extensions and extras of the light.
    pub preserved: Preserved,
}

/// Unsupported extensions and extras of one element, kept as JSON text in canonical form (object
/// keys sorted, no white space). The text is the object as the file gave it, without the
/// extensions this crate reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preserved {
    /// The `extensions` object without the supported extensions, if any remains.
    pub extensions: Option<String>,
    /// The `extras` value, if the file has one.
    pub extras: Option<String>,
}

/// A material of an MTL library or a glTF file. Colors are linear values as written in the file.
/// The OBJ fields are empty for glTF materials and the glTF fields are empty for OBJ materials;
/// an empty field means the file gave no value.
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
    /// The base color factor `[red, green, blue, alpha]` (`pbrMetallicRoughness`).
    pub base_color_factor: Option<[f32; 4]>,
    /// The metalness factor.
    pub metallic_factor: Option<f32>,
    /// The roughness factor.
    pub roughness_factor: Option<f32>,
    /// The base color texture: an image URI or a buffer view of the file.
    pub base_color_texture: Option<ImageSource>,
    /// How the alpha of the material is used.
    pub alpha_mode: Option<AlphaMode>,
    /// The alpha cutoff of the mask mode.
    pub alpha_cutoff: Option<f32>,
    /// Whether both faces are drawn.
    pub double_sided: bool,
    /// Unsupported extensions and extras of the glTF material.
    pub preserved: Preserved,
}

/// Where a texture image comes from: a URI as the file writes it, or a buffer view of the file
/// (the index of a buffer view of the input). Images are never decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// The URI of the image, as written in the file.
    Uri(String),
    /// The index of a buffer view that holds the image.
    BufferView(usize),
}

/// The alpha mode of a glTF material.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlphaMode {
    /// The alpha is ignored.
    #[default]
    Opaque,
    /// Fragments below the cutoff are discarded.
    Mask,
    /// The alpha blends with the background.
    Blend,
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
