//! glTF writing (CORE-IO-024): a GLB, or JSON whose buffer is embedded as a data URI or written
//! beside it.
//!
//! A mesh group becomes a glTF mesh with one primitive per part, and a node that names it draws
//! it; a group shared by several nodes is written once. A part that no group names is written as a
//! mesh of its own under a root node, and a group that no node names gets a root node, so every
//! part is drawn. Positions are written as `f32` with their bounds; normals, the first texture
//! coordinate set and colors follow when the mesh has them; indices are `u16` when the mesh has at
//! most 65 535 vertices, so that no index is 65 535, and `u32` otherwise. The output is
//! deterministic.

use std::collections::{BTreeSet, HashMap};

use arc_base::{Error, Result};
use arc_mesh::{AttributeKey, Channel};
use serde_json::{Map, Value};

use super::{CHUNK_BIN, CHUNK_JSON, GLB_MAGIC, base64_encode, check_hierarchy};
use crate::error::{codes, reasons};
use crate::loss::LossLog;
use crate::model::{
    AlphaMode, Camera, ImageSource, Light, LightKind, Material, MeshGroup, MeshScene, Node,
    Preserved, Projection, Transform, attribute_name,
};
use crate::options::{Encoding, WriteOptions, WriteOutput};
use crate::text::{len_u64, narrow};

/// The extension this writer emits for lights.
const LIGHTS: &str = "KHR_lights_punctual";

/// Writes a scene as glTF JSON or as a GLB, as the encoding and the buffer name choose.
///
/// `Encoding::Ascii` writes JSON with the buffer embedded as a data URI; with
/// `WriteOptions::buffer_name` set, the JSON refers to the buffer by that URI and the bytes are
/// returned in `WriteOutput::buffer`. `Encoding::BinaryLittleEndian` writes one GLB.
///
/// # Errors
///
/// `io.mesh.encoding_unsupported` for big-endian binary or a buffer name with binary encoding,
/// `io.mesh.write_value_invalid` for a value glTF cannot hold (a non-finite number, a reference to
/// a missing mesh, material, camera or light, or a node reached twice) and `base.cancelled`.
pub fn write(scene: &MeshScene, options: &mut WriteOptions<'_>) -> Result<WriteOutput> {
    let layout = match (options.encoding, options.buffer_name.as_deref()) {
        (Encoding::Ascii, None) => Layout::Embedded,
        (Encoding::BinaryLittleEndian, None) => Layout::Glb,
        (Encoding::Ascii, Some(name)) if !name.is_empty() => Layout::Separate(name.to_owned()),
        (Encoding::Ascii, Some(_)) => return Err(write_invalid()),
        _ => return Err(Error::new(codes::MESH_ENCODING_UNSUPPORTED)),
    };
    let mut control = options.control();
    let mut writer = Writer::new(scene);
    let plan = writer.plan()?;
    check_hierarchy(&plan.nodes, &plan.roots, u64::MAX).map_err(|_| write_invalid())?;

    let mut group_out: Vec<Option<usize>> = Vec::with_capacity(plan.groups.len());
    let mut meshes = Vec::new();
    let total = len_u64(plan.groups.len());
    for (index, group) in plan.groups.iter().enumerate() {
        control.step(len_u64(index), total)?;
        if group.primitives.is_empty() {
            group_out.push(None);
            continue;
        }
        let mut primitives = Vec::with_capacity(group.primitives.len());
        for &part in &group.primitives {
            primitives.push(writer.primitive(part, &mut control)?);
        }
        let mut object = Map::new();
        if !group.name.is_empty() {
            object.insert("name".to_owned(), Value::String(group.name.clone()));
        }
        object.insert("primitives".to_owned(), Value::Array(primitives));
        writer.attach(&mut object, &group.preserved, Map::new())?;
        group_out.push(Some(meshes.len()));
        meshes.push(Value::Object(object));
    }

    let mut nodes = Vec::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        nodes.push(writer.node(node, &group_out, scene)?);
    }
    let roots: Vec<Value> = plan
        .roots
        .iter()
        .map(|&root| Value::from(len_u64(root)))
        .collect();
    let mut document = Map::new();
    document.insert("asset".to_owned(), serde_json::json!({ "version": "2.0" }));
    document.insert(
        "scenes".to_owned(),
        Value::Array(vec![serde_json::json!({ "nodes": roots })]),
    );
    document.insert("scene".to_owned(), Value::from(0));
    if !nodes.is_empty() {
        document.insert("nodes".to_owned(), Value::Array(nodes));
    }
    if !meshes.is_empty() {
        document.insert("meshes".to_owned(), Value::Array(meshes));
    }
    writer.finish(&mut document, &layout, scene)?;

    control.finish(len_u64(writer.bin.len()));
    let (bytes, buffer) = match layout {
        Layout::Glb => (writer.glb(document)?, None),
        Layout::Embedded => (to_json(&document)?, None),
        Layout::Separate(_) => (to_json(&document)?, Some(std::mem::take(&mut writer.bin))),
    };
    let report = writer.loss.finish(crate::loss::LossHeader::new(
        "gltf",
        "2.0",
        crate::loss::Direction::Export,
        None,
    ));
    Ok(WriteOutput {
        bytes,
        mtl: None,
        buffer,
        report,
    })
}

/// How the buffer is stored in the output.
enum Layout {
    /// A data URI in the JSON.
    Embedded,
    /// The binary chunk of a GLB.
    Glb,
    /// A file beside the JSON, named by the URI given.
    Separate(String),
}

/// The groups and nodes to write, after the parts and groups that nothing names were given nodes.
struct Plan {
    groups: Vec<MeshGroup>,
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

fn write_invalid() -> Error {
    Error::new(codes::MESH_WRITE_VALUE_INVALID)
}

fn to_json(document: &Map<String, Value>) -> Result<Vec<u8>> {
    serde_json::to_vec(document).map_err(|_| write_invalid())
}

/// A finite value as a JSON number.
fn number(value: f32) -> Result<Value> {
    if !value.is_finite() {
        return Err(write_invalid());
    }
    serde_json::Number::from_f64(f64::from(value))
        .map(Value::Number)
        .ok_or_else(write_invalid)
}

fn numbers(values: &[f32]) -> Result<Value> {
    values
        .iter()
        .map(|&value| number(value))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

/// The bounds of each component of a flat list of values.
fn bounds(values: &[f32], components: usize) -> (Vec<f32>, Vec<f32>) {
    let mut low = vec![f32::INFINITY; components];
    let mut high = vec![f32::NEG_INFINITY; components];
    for element in values.chunks_exact(components) {
        for ((lo, hi), &value) in low.iter_mut().zip(high.iter_mut()).zip(element) {
            *lo = lo.min(value);
            *hi = hi.max(value);
        }
    }
    (low, high)
}

/// The state of one write: the buffer, the accessors and views, and the parts written so far.
struct Writer<'s> {
    scene: &'s MeshScene,
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    parts: Vec<Option<Value>>,
    images: Vec<String>,
    image_of: HashMap<String, usize>,
    material_of: HashMap<&'s str, usize>,
    extensions_used: BTreeSet<String>,
    loss: LossLog,
}

impl<'s> Writer<'s> {
    fn new(scene: &'s MeshScene) -> Self {
        let mut material_of = HashMap::new();
        for (index, material) in scene.materials.iter().enumerate() {
            material_of.entry(material.name.as_str()).or_insert(index);
        }
        Self {
            scene,
            bin: Vec::new(),
            views: Vec::new(),
            accessors: Vec::new(),
            parts: vec![None; scene.meshes.len()],
            images: Vec::new(),
            image_of: HashMap::new(),
            material_of,
            extensions_used: BTreeSet::new(),
            loss: LossLog::default(),
        }
    }

    /// The groups and nodes to write: the scene's, with the parts and groups that nothing names
    /// given their own groups and root nodes.
    fn plan(&mut self) -> Result<Plan> {
        let scene = self.scene;
        let mut groups = Vec::with_capacity(scene.mesh_groups.len());
        let mut claimed = vec![false; scene.meshes.len()];
        for group in &scene.mesh_groups {
            let mut primitives = Vec::new();
            for &part in &group.primitives {
                let mesh = scene.meshes.get(part).ok_or_else(write_invalid)?;
                if mesh.mesh.triangle_count() == 0 {
                    self.loss.add(reasons::ELEMENT_SKIPPED, "part", None, 1);
                    continue;
                }
                claimed[part] = true;
                primitives.push(part);
            }
            groups.push(MeshGroup {
                name: group.name.clone(),
                primitives,
                preserved: group.preserved.clone(),
            });
        }
        for (index, part) in scene.meshes.iter().enumerate() {
            if claimed[index] {
                continue;
            }
            if part.mesh.triangle_count() == 0 {
                self.loss.add(reasons::ELEMENT_SKIPPED, "part", None, 1);
            } else {
                groups.push(MeshGroup {
                    name: part.name.clone(),
                    primitives: vec![index],
                    preserved: Preserved::default(),
                });
            }
        }

        let mut nodes = scene.nodes.clone();
        let mut roots = scene.roots.clone();
        if roots.iter().any(|&root| root >= nodes.len()) {
            return Err(write_invalid());
        }
        let mut referenced = vec![false; groups.len()];
        for node in &nodes {
            if let Some(group) = node.mesh {
                if group >= scene.mesh_groups.len() {
                    return Err(write_invalid());
                }
                *referenced.get_mut(group).ok_or_else(write_invalid)? = true;
            }
        }
        for (group, used) in referenced.iter().enumerate() {
            if !used {
                nodes.push(Node {
                    mesh: Some(group),
                    ..Node::default()
                });
                roots.push(nodes.len() - 1);
            }
        }
        Ok(Plan {
            groups,
            nodes,
            roots,
        })
    }

    /// Appends an accessor over `bytes`, in a buffer view of its own. The view starts on a
    /// four-byte boundary; the bounds are written when given.
    fn push(
        &mut self,
        bytes: &[u8],
        component: u64,
        count: usize,
        kind: &str,
        bounds: Option<(Vec<f32>, Vec<f32>)>,
    ) -> Result<usize> {
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let padded = self.bin.len().next_multiple_of(4);
        self.bin.resize(padded, 0);
        let view = self.views.len();
        self.views.push(serde_json::json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
        }));
        let mut accessor = serde_json::json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": kind,
        });
        if let (Some((low, high)), Some(object)) = (bounds, accessor.as_object_mut()) {
            object.insert("min".to_owned(), numbers(&low)?);
            object.insert("max".to_owned(), numbers(&high)?);
        }
        self.accessors.push(accessor);
        Ok(self.accessors.len() - 1)
    }

    /// The primitive of a part, written once however many groups name it.
    fn primitive(
        &mut self,
        part: usize,
        control: &mut crate::options::Control<'_>,
    ) -> Result<Value> {
        if let Some(Some(done)) = self.parts.get(part) {
            return Ok(done.clone());
        }
        control.step(len_u64(part), len_u64(self.parts.len()))?;
        let scene = self.scene;
        let source = scene.meshes.get(part).ok_or_else(write_invalid)?;
        let mesh = &source.mesh;
        let vertices = mesh.vertex_count();
        let mut attributes = Map::new();

        let mut positions = Vec::with_capacity(vertices * 3);
        let mut moved = 0_u64;
        for point in mesh.positions() {
            for value in [point.x, point.y, point.z] {
                let single = narrow(value);
                if f64::from(single) != value {
                    moved += 1;
                }
                positions.push(single);
            }
        }
        let bytes = floats_bytes(&positions)?;
        let (low, high) = bounds(&positions, 3);
        let position = self.push(&bytes, 5126, vertices, "VEC3", Some((low, high)))?;
        attributes.insert("POSITION".to_owned(), Value::from(len_u64(position)));
        self.loss
            .attribute(reasons::PRECISION_REDUCED, "position", moved);

        for (key, name, components, kind) in [
            (AttributeKey::Normal, "NORMAL", 3, "VEC3"),
            (AttributeKey::Uv(0), "TEXCOORD_0", 2, "VEC2"),
            (AttributeKey::Color, "COLOR_0", 4, "VEC4"),
        ] {
            let values: Option<Vec<f32>> = match (mesh.channel(&key), components) {
                (Some(Channel::Vec3(v)), 3) => Some(v.iter().flatten().copied().collect()),
                (Some(Channel::Vec2(v)), 2) => Some(v.iter().flatten().copied().collect()),
                (Some(Channel::Vec4(v)), 4) => Some(v.iter().flatten().copied().collect()),
                _ => None,
            };
            if let Some(values) = values {
                let bytes = floats_bytes(&values)?;
                let accessor = self.push(&bytes, 5126, vertices, kind, None)?;
                attributes.insert(name.to_owned(), Value::from(len_u64(accessor)));
            }
        }
        for key in mesh.channels().keys() {
            if !matches!(
                key,
                AttributeKey::Normal | AttributeKey::Uv(0) | AttributeKey::Color
            ) {
                self.loss.attribute(
                    reasons::ATTRIBUTE_DROPPED,
                    attribute_name(key),
                    len_u64(vertices),
                );
            }
        }

        let mut indices: Vec<u32> = Vec::with_capacity(mesh.triangle_count() * 3);
        for triangle in mesh.triangles() {
            indices.extend_from_slice(triangle);
        }
        // glTF does not allow 65 535 as an unsigned short index, so 16-bit indices need fewer.
        let (component, bytes) = if vertices <= 65_535 {
            let narrow_indices: Vec<u16> = indices
                .iter()
                .map(|&index| u16::try_from(index).map_err(|_| write_invalid()))
                .collect::<Result<_>>()?;
            (
                5123,
                narrow_indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
        } else {
            (
                5125,
                indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
        };
        let index = self.push(&bytes, component, indices.len(), "SCALAR", None)?;

        let mut primitive = Map::new();
        primitive.insert("attributes".to_owned(), Value::Object(attributes));
        primitive.insert("indices".to_owned(), Value::from(len_u64(index)));
        if let Some(name) = &source.material {
            let material = *self
                .material_of
                .get(name.as_str())
                .ok_or_else(write_invalid)?;
            primitive.insert("material".to_owned(), Value::from(len_u64(material)));
        }
        self.attach(&mut primitive, &source.preserved, Map::new())?;
        let value = Value::Object(primitive);
        if let Some(slot) = self.parts.get_mut(part) {
            *slot = Some(value.clone());
        }
        Ok(value)
    }

    /// Adds the preserved extensions and extras of an element, with the supported extensions it
    /// carries, and records the extension names for `extensionsUsed`.
    fn attach(
        &mut self,
        object: &mut Map<String, Value>,
        preserved: &Preserved,
        supported: Map<String, Value>,
    ) -> Result<()> {
        let mut extensions = match &preserved.extensions {
            None => Map::new(),
            Some(text) => match serde_json::from_str::<Value>(text).map_err(|_| write_invalid())? {
                Value::Object(map) => map,
                _ => return Err(write_invalid()),
            },
        };
        extensions.extend(supported);
        for name in extensions.keys() {
            self.extensions_used.insert(name.clone());
        }
        if !extensions.is_empty() {
            object.insert("extensions".to_owned(), Value::Object(extensions));
        }
        if let Some(text) = &preserved.extras {
            let extras = serde_json::from_str::<Value>(text).map_err(|_| write_invalid())?;
            object.insert("extras".to_owned(), extras);
        }
        Ok(())
    }

    fn node(
        &mut self,
        node: &Node,
        group_out: &[Option<usize>],
        scene: &MeshScene,
    ) -> Result<Value> {
        let mut object = Map::new();
        if !node.name.is_empty() {
            object.insert("name".to_owned(), Value::String(node.name.clone()));
        }
        match node.transform {
            Transform::Matrix(matrix) => {
                object.insert("matrix".to_owned(), numbers(&matrix)?);
            }
            Transform::Trs {
                translation,
                rotation,
                scale,
            } => {
                if translation != [0.0; 3] {
                    object.insert("translation".to_owned(), numbers(&translation)?);
                }
                if rotation != [0.0, 0.0, 0.0, 1.0] {
                    object.insert("rotation".to_owned(), numbers(&rotation)?);
                }
                if scale != [1.0; 3] {
                    object.insert("scale".to_owned(), numbers(&scale)?);
                }
            }
        }
        if let Some(Some(mesh)) = node.mesh.and_then(|group| group_out.get(group)) {
            object.insert("mesh".to_owned(), Value::from(len_u64(*mesh)));
        }
        if !node.children.is_empty() {
            let children: Vec<Value> = node
                .children
                .iter()
                .map(|&child| Value::from(len_u64(child)))
                .collect();
            object.insert("children".to_owned(), Value::Array(children));
        }
        if let Some(camera) = node.camera {
            if camera >= scene.cameras.len() {
                return Err(write_invalid());
            }
            object.insert("camera".to_owned(), Value::from(len_u64(camera)));
        }
        let mut supported = Map::new();
        if let Some(light) = node.light {
            if light >= scene.lights.len() {
                return Err(write_invalid());
            }
            supported.insert(LIGHTS.to_owned(), serde_json::json!({ "light": light }));
            self.extensions_used.insert(LIGHTS.to_owned());
        }
        self.attach(&mut object, &node.preserved, supported)?;
        Ok(Value::Object(object))
    }

    fn image(&mut self, source: &ImageSource) -> Option<usize> {
        match source {
            ImageSource::Uri(uri) => {
                if let Some(&index) = self.image_of.get(uri) {
                    return Some(index);
                }
                let index = self.images.len();
                self.images.push(uri.clone());
                self.image_of.insert(uri.clone(), index);
                Some(index)
            }
            ImageSource::BufferView(_) => {
                self.loss.add(reasons::TEXTURE_DROPPED, "image", None, 1);
                None
            }
        }
    }

    fn material(&mut self, material: &Material) -> Result<Value> {
        let mut object = Map::new();
        object.insert("name".to_owned(), Value::String(material.name.clone()));
        if material.diffuse.is_some()
            || material.ambient.is_some()
            || material.specular.is_some()
            || material.opacity.is_some()
            || !material.textures.is_empty()
        {
            self.loss
                .add(reasons::PROPERTY_SKIPPED, "material", None, 1);
        }
        let mut pbr = Map::new();
        if let Some(factor) = material.base_color_factor {
            pbr.insert("baseColorFactor".to_owned(), numbers(&factor)?);
        }
        if let Some(metallic) = material.metallic_factor {
            pbr.insert("metallicFactor".to_owned(), number(metallic)?);
        }
        if let Some(roughness) = material.roughness_factor {
            pbr.insert("roughnessFactor".to_owned(), number(roughness)?);
        }
        if let Some(index) = material
            .base_color_texture
            .as_ref()
            .and_then(|source| self.image(source))
        {
            pbr.insert(
                "baseColorTexture".to_owned(),
                serde_json::json!({ "index": index }),
            );
        }
        if !pbr.is_empty() {
            object.insert("pbrMetallicRoughness".to_owned(), Value::Object(pbr));
        }
        if let Some(mode) = material.alpha_mode {
            let text = match mode {
                AlphaMode::Opaque => "OPAQUE",
                AlphaMode::Mask => "MASK",
                AlphaMode::Blend => "BLEND",
            };
            object.insert("alphaMode".to_owned(), Value::String(text.to_owned()));
        }
        if let Some(cutoff) = material.alpha_cutoff {
            object.insert("alphaCutoff".to_owned(), number(cutoff)?);
        }
        if material.double_sided {
            object.insert("doubleSided".to_owned(), Value::Bool(true));
        }
        self.attach(&mut object, &material.preserved, Map::new())?;
        Ok(Value::Object(object))
    }

    fn camera(&mut self, camera: &Camera) -> Result<Value> {
        let mut object = Map::new();
        if !camera.name.is_empty() {
            object.insert("name".to_owned(), Value::String(camera.name.clone()));
        }
        match camera.projection {
            Projection::Perspective {
                yfov,
                aspect_ratio,
                znear,
                zfar,
            } => {
                let mut body = Map::new();
                body.insert("yfov".to_owned(), number(yfov)?);
                if let Some(ratio) = aspect_ratio {
                    body.insert("aspectRatio".to_owned(), number(ratio)?);
                }
                body.insert("znear".to_owned(), number(znear)?);
                if let Some(far) = zfar {
                    body.insert("zfar".to_owned(), number(far)?);
                }
                object.insert("type".to_owned(), Value::String("perspective".to_owned()));
                object.insert("perspective".to_owned(), Value::Object(body));
            }
            Projection::Orthographic {
                xmag,
                ymag,
                znear,
                zfar,
            } => {
                let body = serde_json::json!({
                    "xmag": number(xmag)?,
                    "ymag": number(ymag)?,
                    "znear": number(znear)?,
                    "zfar": number(zfar)?,
                });
                object.insert("type".to_owned(), Value::String("orthographic".to_owned()));
                object.insert("orthographic".to_owned(), body);
            }
        }
        self.attach(&mut object, &camera.preserved, Map::new())?;
        Ok(Value::Object(object))
    }

    fn light(&mut self, light: &Light) -> Result<Value> {
        let mut object = Map::new();
        if !light.name.is_empty() {
            object.insert("name".to_owned(), Value::String(light.name.clone()));
        }
        let kind = match light.kind {
            LightKind::Directional => "directional",
            LightKind::Point => "point",
            LightKind::Spot { .. } => "spot",
        };
        object.insert("type".to_owned(), Value::String(kind.to_owned()));
        object.insert("color".to_owned(), numbers(&light.color)?);
        object.insert("intensity".to_owned(), number(light.intensity)?);
        if let Some(range) = light.range {
            object.insert("range".to_owned(), number(range)?);
        }
        if let LightKind::Spot {
            inner_cone_angle,
            outer_cone_angle,
        } = light.kind
        {
            object.insert(
                "spot".to_owned(),
                serde_json::json!({
                    "innerConeAngle": number(inner_cone_angle)?,
                    "outerConeAngle": number(outer_cone_angle)?,
                }),
            );
        }
        self.attach(&mut object, &light.preserved, Map::new())?;
        Ok(Value::Object(object))
    }

    /// Writes the materials, cameras, lights, buffer and the remaining top-level arrays.
    fn finish(
        &mut self,
        document: &mut Map<String, Value>,
        layout: &Layout,
        scene: &MeshScene,
    ) -> Result<()> {
        let mut materials = Vec::with_capacity(scene.materials.len());
        for material in &scene.materials {
            materials.push(self.material(material)?);
        }
        if !materials.is_empty() {
            document.insert("materials".to_owned(), Value::Array(materials));
        }
        if !self.images.is_empty() {
            let images: Vec<Value> = self
                .images
                .iter()
                .map(|uri| serde_json::json!({ "uri": uri }))
                .collect();
            let textures: Vec<Value> = (0..self.images.len())
                .map(|index| serde_json::json!({ "source": index }))
                .collect();
            document.insert("images".to_owned(), Value::Array(images));
            document.insert("textures".to_owned(), Value::Array(textures));
        }
        let mut cameras = Vec::with_capacity(scene.cameras.len());
        for camera in &scene.cameras {
            cameras.push(self.camera(camera)?);
        }
        if !cameras.is_empty() {
            document.insert("cameras".to_owned(), Value::Array(cameras));
        }
        let mut lights = Vec::with_capacity(scene.lights.len());
        for light in &scene.lights {
            lights.push(self.light(light)?);
        }
        let mut root_extensions = Map::new();
        if !lights.is_empty() {
            root_extensions.insert(LIGHTS.to_owned(), serde_json::json!({ "lights": lights }));
            self.extensions_used.insert(LIGHTS.to_owned());
        }
        let mut attached = Map::new();
        self.attach(&mut attached, &scene.preserved, root_extensions)?;
        for (key, value) in attached {
            document.insert(key, value);
        }
        if !self.extensions_used.is_empty() {
            let used: Vec<Value> = self
                .extensions_used
                .iter()
                .map(|name| Value::String(name.clone()))
                .collect();
            document.insert("extensionsUsed".to_owned(), Value::Array(used));
        }
        if !self.views.is_empty() {
            document.insert("bufferViews".to_owned(), Value::Array(self.views.clone()));
        }
        if !self.accessors.is_empty() {
            document.insert("accessors".to_owned(), Value::Array(self.accessors.clone()));
        }
        if !self.bin.is_empty() {
            let length = Value::from(len_u64(self.bin.len()));
            let buffer = match layout {
                Layout::Embedded => serde_json::json!({
                    "byteLength": length,
                    "uri": format!(
                        "data:application/octet-stream;base64,{}",
                        base64_encode(&self.bin)
                    ),
                }),
                Layout::Glb => serde_json::json!({ "byteLength": length }),
                Layout::Separate(name) => serde_json::json!({
                    "byteLength": length,
                    "uri": name,
                }),
            };
            document.insert("buffers".to_owned(), Value::Array(vec![buffer]));
        }
        Ok(())
    }

    /// The GLB container: a JSON chunk padded with spaces and a binary chunk padded with zeros.
    fn glb(&self, document: Map<String, Value>) -> Result<Vec<u8>> {
        let mut json = to_json(&document)?;
        json.resize(json.len().next_multiple_of(4), b' ');
        let mut out = Vec::with_capacity(12 + 8 + json.len() + 8 + self.bin.len());
        out.extend_from_slice(GLB_MAGIC);
        out.extend_from_slice(&2_u32.to_le_bytes());
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(&len_u32(json.len())?.to_le_bytes());
        out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        out.extend_from_slice(&json);
        if !self.bin.is_empty() {
            out.extend_from_slice(&len_u32(self.bin.len())?.to_le_bytes());
            out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
            out.extend_from_slice(&self.bin);
        }
        let total = len_u32(out.len())?;
        if let Some(header) = out.get_mut(8..12) {
            header.copy_from_slice(&total.to_le_bytes());
        }
        Ok(out)
    }
}

fn len_u32(length: usize) -> Result<u32> {
    u32::try_from(length).map_err(|_| write_invalid())
}

/// The little-endian bytes of single-precision values.
fn floats_bytes(values: &[f32]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for &value in values {
        if !value.is_finite() {
            return Err(write_invalid());
        }
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(out)
}
