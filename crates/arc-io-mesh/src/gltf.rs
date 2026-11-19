//! glTF 2.0 and GLB reading (CORE-IO-023) and writing (CORE-IO-024).
//!
//! [`read`] takes the bytes of a `.gltf` JSON file or of a `.glb` container. The JSON is checked
//! for nesting depth before it is parsed, with `serde_json`. Buffers come from the GLB binary
//! chunk, from `data:` URIs (base64, decoded here) or from the caller's resolver for any other
//! URI; the crate never opens a file. Accessors cover every component type and element type,
//! byte strides, normalized integers and sparse storage; `min` and `max` are not read. Meshes keep
//! the triangle, strip and fan primitives, with the positions, normals, first texture coordinates
//! and colors. The scene hierarchy, cameras, `KHR_lights_punctual` lights and the PBR fields of
//! materials are kept as plain data. Other extensions and extras stay as JSON text on the element
//! they belong to. [`write`] (in `gltf/write.rs`) produces the reverse.
//!
//! Every length, count, offset and index of the file is checked with checked arithmetic against
//! the buffer it points into and against the limits before anything is read or allocated; no
//! input panics.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_4;

use arc_base::math::Point3;
use arc_base::{Error, Limits, Result};
use arc_mesh::{AttributeKey, Channel};
use serde_json::{Map, Value};

use crate::error::{codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog};
use crate::model::{
    AlphaMode, Camera, ImageSource, Light, LightKind, Material, MeshGroup, MeshPart, MeshScene,
    Node, Preserved, Projection, Transform, build_mesh,
};
use crate::options::{Control, ReadOptions, ReadOutput};
use crate::text::{self, len_u64, narrow};

mod write;

pub use write::write;

/// The extensions this reader and writer interpret; every other extension is preserved.
const SUPPORTED_EXTENSIONS: &[&str] = &["KHR_lights_punctual"];
const LIGHTS: &str = "KHR_lights_punctual";
const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
/// The glTF primitive modes this reader converts: triangles, triangle strip and triangle fan.
const MODE_TRIANGLES: u64 = 4;
const MODE_STRIP: u64 = 5;
const MODE_FAN: u64 = 6;
/// Elements gathered between two progress and cancellation steps inside one accessor.
const GATHER_STEP: usize = 65_536;

/// Reads a glTF JSON file or a GLB container.
///
/// `resolver` is called with each URI that is neither a `data:` URI nor the GLB binary chunk, as
/// the file writes it (percent-encoding is not decoded), and returns the bytes of the resource or
/// `None` when it has none. The bytes of the resolver and the decoded data URIs count against
/// `Limits::decompressed_bytes`, and so does the primitive data read from accessors, which is
/// charged before it is decoded.
///
/// # Errors
///
/// `io.limit_exceeded` naming the limit, `io.mesh.header_invalid` for a GLB header or an asset
/// version other than 2.x, `io.mesh.extension_unsupported` for a required extension that is not
/// supported, `io.mesh.resource_unavailable` for a buffer the resolver did not supply,
/// `io.mesh.truncated` when data ends before a declared length, `io.mesh.reference_invalid` for an
/// index that names nothing, or a node that is reached twice, `io.mesh.structure_invalid`,
/// `io.mesh.value_invalid` and `base.cancelled`.
pub fn read(
    bytes: &[u8],
    resolver: impl Fn(&str) -> Option<Vec<u8>>,
    options: &mut ReadOptions<'_>,
) -> Result<ReadOutput> {
    let limits = options.limits;
    text::check_input(bytes.len(), &limits)?;
    let mut control = options.control();
    let (json, bin, skipped) = split_container(bytes)?;
    check_nesting(json, limits.nesting_depth)?;
    let document: Value = serde_json::from_slice(json).map_err(|_| invalid())?;
    let root = object(&document)?;
    let mut reader = Reader::new(limits, bin);
    reader
        .loss
        .add(reasons::ELEMENT_SKIPPED, "chunk", None, skipped);
    let scene = reader.run(root, &resolver, &mut control)?;
    control.finish(len_u64(bytes.len()));
    let loss = std::mem::take(&mut reader.loss);
    let report = loss.finish(LossHeader::new(
        "gltf",
        "2.0",
        Direction::Import,
        Some(limits),
    ));
    Ok(ReadOutput { scene, report })
}

fn invalid() -> Error {
    Error::new(codes::MESH_STRUCTURE_INVALID)
}

fn value_invalid() -> Error {
    Error::new(codes::MESH_VALUE_INVALID)
}

fn truncated() -> Error {
    Error::new(codes::MESH_TRUNCATED)
}

fn reference() -> Error {
    Error::new(codes::MESH_REFERENCE_INVALID)
}

/// The JSON bytes, the first binary chunk and the number of skipped chunks of a file.
type Container<'a> = (&'a [u8], Option<&'a [u8]>, u64);

/// Splits a GLB container into its JSON chunk and its first binary chunk; a file without the
/// GLB magic is JSON and has no binary chunk.
fn split_container(bytes: &[u8]) -> Result<Container<'_>> {
    if bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
        return Ok((bytes, None, 0));
    }
    let header = bytes.get(..12).ok_or_else(truncated)?;
    let version = u32_at(header, 4)?;
    if version != 2 {
        return Err(Error::new(codes::MESH_HEADER_INVALID));
    }
    let total = usize::try_from(u32_at(header, 8)?).map_err(|_| truncated())?;
    if total < 12 {
        return Err(Error::new(codes::MESH_HEADER_INVALID));
    }
    let body = bytes.get(..total).ok_or_else(truncated)?;
    let mut json = None;
    let mut bin = None;
    let mut skipped = 0_u64;
    let mut at = 12;
    while at < total {
        let length = usize::try_from(u32_at(body, at)?).map_err(|_| truncated())?;
        let kind = u32_at(body, at + 4)?;
        if !length.is_multiple_of(4) {
            return Err(invalid());
        }
        let start = at.checked_add(8).ok_or_else(truncated)?;
        let end = start.checked_add(length).ok_or_else(truncated)?;
        let data = body.get(start..end).ok_or_else(truncated)?;
        match kind {
            CHUNK_JSON if at == 12 => json = Some(data),
            CHUNK_BIN if bin.is_none() && json.is_some() => bin = Some(data),
            // The first chunk must be the JSON; unknown and repeated chunks are ignored.
            _ if json.is_none() => return Err(invalid()),
            _ => skipped = skipped.saturating_add(1),
        }
        at = end;
    }
    json.map(|json| (json, bin, skipped)).ok_or_else(invalid)
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let end = at.checked_add(4).ok_or_else(truncated)?;
    let raw = bytes.get(at..end).ok_or_else(truncated)?;
    Ok(u32::from_le_bytes(array(raw)?))
}

fn array<const N: usize>(raw: &[u8]) -> Result<[u8; N]> {
    <[u8; N]>::try_from(raw).map_err(|_| truncated())
}

/// Fails with `io.limit_exceeded` when the JSON nests deeper than `depth`. Brackets inside
/// strings do not count; this runs before the parser so that its recursion is bounded.
fn check_nesting(json: &[u8], depth: u32) -> Result<()> {
    let max = u64::from(depth);
    let mut current: u64 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in json {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                current += 1;
                if current > max {
                    return Err(limit("nesting_depth", current));
                }
            }
            b'}' | b']' => current = current.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// The walk of the node hierarchy failed: a node is reached twice (a cycle included) or does not
/// exist, or the hierarchy is deeper than allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HierarchyFault {
    /// A node is reached twice or does not exist.
    Reference,
    /// A node is deeper than the allowed depth; the value is the depth reached.
    Depth(u64),
}

/// Walks the hierarchy from the roots, then from every node that no node names as a child and the
/// walk has not reached. Those extra roots are validated but are not roots of the scene. Fails
/// when a node is reached twice, does not exist or is deeper than `max_depth`, or when a node is
/// still unreached, which means it lies on a cycle. Depth counts the roots as zero.
pub(crate) fn check_hierarchy(
    nodes: &[Node],
    roots: &[usize],
    max_depth: u64,
) -> std::result::Result<(), HierarchyFault> {
    let mut seen = vec![false; nodes.len()];
    walk_hierarchy(nodes, roots, max_depth, &mut seen)?;
    let mut named = vec![false; nodes.len()];
    for node in nodes {
        for &child in &node.children {
            if let Some(flag) = named.get_mut(child) {
                *flag = true;
            }
        }
    }
    let extra: Vec<usize> = seen
        .iter()
        .zip(&named)
        .enumerate()
        .filter(|(_, (reached, named))| !**reached && !**named)
        .map(|(index, _)| index)
        .collect();
    walk_hierarchy(nodes, &extra, max_depth, &mut seen)?;
    if seen.iter().all(|&reached| reached) {
        Ok(())
    } else {
        Err(HierarchyFault::Reference)
    }
}

/// Walks the nodes below `roots`, marking each in `seen`. The walk is iterative, so depth never
/// drives recursion.
fn walk_hierarchy(
    nodes: &[Node],
    roots: &[usize],
    max_depth: u64,
    seen: &mut [bool],
) -> std::result::Result<(), HierarchyFault> {
    let mut stack: Vec<(usize, u64)> = roots.iter().rev().map(|&root| (root, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        let reached = seen.get_mut(index).ok_or(HierarchyFault::Reference)?;
        if *reached {
            return Err(HierarchyFault::Reference);
        }
        *reached = true;
        if depth > max_depth {
            return Err(HierarchyFault::Depth(depth));
        }
        let node = nodes.get(index).ok_or(HierarchyFault::Reference)?;
        stack.extend(node.children.iter().rev().map(|&child| (child, depth + 1)));
    }
    Ok(())
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(invalid)
}

/// The array under `key`; a missing key is an empty array.
fn array_of<'v>(obj: &'v Map<String, Value>, key: &str) -> Result<&'v [Value]> {
    match obj.get(key) {
        None => Ok(&[]),
        Some(value) => value.as_array().map(Vec::as_slice).ok_or_else(invalid),
    }
}

/// An unsigned integer; a missing value is `None`.
fn index_of(value: Option<&Value>) -> Result<Option<usize>> {
    match value {
        None => Ok(None),
        Some(value) => {
            let number = value.as_u64().ok_or_else(invalid)?;
            usize::try_from(number).map(Some).map_err(|_| invalid())
        }
    }
}

/// A required unsigned integer under `key`.
fn required_index(obj: &Map<String, Value>, key: &str) -> Result<usize> {
    index_of(obj.get(key))?.ok_or_else(invalid)
}

/// An optional text under `key`, which must not exceed `Limits::string_bytes`.
fn text_of(obj: &Map<String, Value>, key: &str, limits: &Limits) -> Result<String> {
    match obj.get(key) {
        None => Ok(String::new()),
        Some(value) => {
            let text = value.as_str().ok_or_else(invalid)?;
            check_text(text, limits)?;
            Ok(text.to_owned())
        }
    }
}

fn check_text(text: &str, limits: &Limits) -> Result<()> {
    let length = len_u64(text.len());
    if length > limits.string_bytes {
        Err(limit("string_bytes", length))
    } else {
        Ok(())
    }
}

/// A number stored as `f32`: it must be finite after the narrowing, since a large `f64` becomes
/// infinite.
fn number_value(value: &Value) -> Result<f32> {
    let number = narrow(value.as_f64().ok_or_else(value_invalid)?);
    if number.is_finite() {
        Ok(number)
    } else {
        Err(value_invalid())
    }
}

fn number_of(obj: &Map<String, Value>, key: &str) -> Result<Option<f32>> {
    obj.get(key).map(number_value).transpose()
}

fn numbers<const N: usize>(value: &Value) -> Result<[f32; N]> {
    let items = value.as_array().ok_or_else(invalid)?;
    let mut out = [0.0_f32; N];
    if items.len() != N {
        return Err(invalid());
    }
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = number_value(item)?;
    }
    Ok(out)
}

fn numbers_of<const N: usize>(obj: &Map<String, Value>, key: &str) -> Result<Option<[f32; N]>> {
    obj.get(key).map(numbers::<N>).transpose()
}

fn flag_of(obj: &Map<String, Value>, key: &str) -> Result<bool> {
    match obj.get(key) {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(invalid),
    }
}

/// The canonical JSON text of a value.
fn canonical(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid())
}

/// The component types of accessors (glTF table 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Component {
    I8,
    U8,
    I16,
    U16,
    U32,
    F32,
}

impl Component {
    fn from_code(code: u64) -> Result<Self> {
        match code {
            5120 => Ok(Self::I8),
            5121 => Ok(Self::U8),
            5122 => Ok(Self::I16),
            5123 => Ok(Self::U16),
            5125 => Ok(Self::U32),
            5126 => Ok(Self::F32),
            _ => Err(value_invalid()),
        }
    }

    /// The size of one component in bytes.
    pub(crate) fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::U32 | Self::F32 => 4,
        }
    }

    /// Whether the component is an integer, the only kind that may be normalized.
    fn is_integer(self) -> bool {
        !matches!(self, Self::U32 | Self::F32)
    }
}

/// The element types of accessors (glTF table 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Scalar,
    Vec2,
    Vec3,
    Vec4,
    Mat2,
    Mat3,
    Mat4,
}

impl Kind {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "SCALAR" => Ok(Self::Scalar),
            "VEC2" => Ok(Self::Vec2),
            "VEC3" => Ok(Self::Vec3),
            "VEC4" => Ok(Self::Vec4),
            "MAT2" => Ok(Self::Mat2),
            "MAT3" => Ok(Self::Mat3),
            "MAT4" => Ok(Self::Mat4),
            _ => Err(invalid()),
        }
    }

    /// The number of components of one element.
    pub(crate) fn components(self) -> usize {
        match self {
            Self::Scalar => 1,
            Self::Vec2 => 2,
            Self::Vec3 => 3,
            Self::Vec4 | Self::Mat2 => 4,
            Self::Mat3 => 9,
            Self::Mat4 => 16,
        }
    }

    fn is_matrix(self) -> bool {
        matches!(self, Self::Mat2 | Self::Mat3 | Self::Mat4)
    }

    /// The size of one element in bytes. Each column of a matrix starts on a four-byte boundary,
    /// so a column of byte or short components is padded.
    pub(crate) fn element_size(self, component: Component) -> usize {
        let size = component.size();
        match self {
            Self::Mat2 => 2 * (2 * size).next_multiple_of(4),
            Self::Mat3 => 3 * (3 * size).next_multiple_of(4),
            Self::Mat4 => 4 * (4 * size),
            other => other.components() * size,
        }
    }
}

/// A `bufferView`: a range of a buffer, with an optional stride between elements.
#[derive(Debug, Clone, Copy)]
struct View {
    buffer: usize,
    offset: usize,
    length: usize,
    stride: Option<usize>,
}

/// Where the data of a sparse accessor part lives.
#[derive(Debug, Clone, Copy)]
struct Span {
    view: usize,
    offset: usize,
}

/// The sparse part of an accessor: elements that replace the base data.
#[derive(Debug, Clone, Copy)]
struct Sparse {
    count: usize,
    indices: Span,
    index_component: Component,
    values: Span,
}

/// An `accessor`, as declared. Data is read on demand.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Accessor {
    view: Option<usize>,
    offset: usize,
    component: Component,
    normalized: bool,
    kind: Kind,
    count: usize,
    sparse: Option<Sparse>,
}

/// A buffer: its bytes and its declared length.
struct Buffer<'a> {
    data: Cow<'a, [u8]>,
    length: usize,
}

/// The state of one read.
struct Reader<'a> {
    limits: Limits,
    bin: Option<&'a [u8]>,
    buffers: Vec<Buffer<'a>>,
    views: Vec<View>,
    accessors: Vec<Accessor>,
    materials: Vec<Material>,
    parts: Vec<MeshPart>,
    groups: Vec<MeshGroup>,
    records: u64,
    owned_bytes: u64,
    /// The bytes of primitive data charged so far, against `Limits::decompressed_bytes`.
    decoded: u64,
    loss: LossLog,
}

impl<'a> Reader<'a> {
    fn new(limits: Limits, bin: Option<&'a [u8]>) -> Self {
        Self {
            limits,
            bin,
            buffers: Vec::new(),
            views: Vec::new(),
            accessors: Vec::new(),
            materials: Vec::new(),
            parts: Vec::new(),
            groups: Vec::new(),
            records: 0,
            owned_bytes: 0,
            decoded: 0,
            loss: LossLog::default(),
        }
    }

    /// Charges the bytes a primitive will hold, before any of its data is decoded.
    fn charge_decoded(&mut self, bytes: u64) -> Result<()> {
        self.decoded = self.decoded.saturating_add(bytes);
        if self.decoded > self.limits.decompressed_bytes {
            Err(limit("decompressed_bytes", self.decoded))
        } else {
            Ok(())
        }
    }

    /// Counts one record against `Limits::record_count`.
    fn record(&mut self) -> Result<()> {
        self.records = self.records.saturating_add(1);
        if self.records > self.limits.record_count {
            Err(limit("record_count", self.records))
        } else {
            Ok(())
        }
    }

    /// Checks an array length against `Limits::array_length`.
    fn check_count(&self, count: usize) -> Result<()> {
        let count = len_u64(count);
        if count > self.limits.array_length {
            Err(limit("array_length", count))
        } else {
            Ok(())
        }
    }

    /// Reads the extensions and extras of an element. Supported extensions named in `consumed`
    /// are returned; the others and the extras are kept as JSON text when `keep` is set, and
    /// reported as dropped otherwise.
    fn extensions_of(
        &mut self,
        obj: &Map<String, Value>,
        kind: &str,
        consumed: &[&str],
        keep: bool,
    ) -> Result<(Map<String, Value>, Preserved)> {
        let mut taken = Map::new();
        let mut kept = Map::new();
        if let Some(extensions) = obj.get("extensions") {
            for (key, value) in object(extensions)? {
                if consumed.contains(&key.as_str()) {
                    taken.insert(key.clone(), value.clone());
                } else {
                    kept.insert(key.clone(), value.clone());
                }
            }
        }
        let extras = obj.get("extras");
        let mut preserved = Preserved::default();
        if keep {
            if !kept.is_empty() {
                let count = len_u64(kept.len());
                self.loss.add(
                    reasons::EXTENSION_PRESERVED,
                    &format!("{kind} extension"),
                    None,
                    count,
                );
                preserved.extensions = Some(canonical(&Value::Object(kept))?);
            }
            if let Some(extras) = extras {
                self.loss.add(
                    reasons::EXTENSION_PRESERVED,
                    &format!("{kind} extras"),
                    None,
                    1,
                );
                preserved.extras = Some(canonical(extras)?);
            }
        } else {
            let dropped = len_u64(kept.len()) + u64::from(extras.is_some());
            self.loss.add(
                reasons::PROPERTY_SKIPPED,
                &format!("{kind} extension"),
                None,
                dropped,
            );
        }
        Ok((taken, preserved))
    }

    fn run(
        &mut self,
        root: &'a Map<String, Value>,
        resolver: &impl Fn(&str) -> Option<Vec<u8>>,
        control: &mut Control<'_>,
    ) -> Result<MeshScene> {
        let asset = object(root.get("asset").ok_or_else(invalid)?)?;
        let version = asset
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if !version.starts_with("2.") {
            return Err(Error::new(codes::MESH_HEADER_INVALID));
        }
        self.extensions_of(asset, "asset", &[], false)?;
        for required in array_of(root, "extensionsRequired")? {
            let name = required.as_str().ok_or_else(invalid)?;
            if !SUPPORTED_EXTENSIONS.contains(&name) {
                return Err(Error::new(codes::MESH_EXTENSION_UNSUPPORTED)
                    .with_arg("extension", name.to_owned()));
            }
        }
        let (root_lights, document) = self.extensions_of(root, "document", &[LIGHTS], true)?;

        self.read_buffers(root, resolver, control)?;
        self.read_views(root)?;
        self.read_accessors(root)?;
        self.note_skipped(root)?;
        let materials = self.read_materials(root)?;
        self.materials = materials;
        for value in array_of(root, "meshes")? {
            control.step(len_u64(self.groups.len()), 0)?;
            let group = self.read_mesh(object(value)?, control)?;
            self.groups.push(group);
        }
        let lights = self.read_lights(&root_lights)?;
        let cameras = self.read_cameras(root)?;
        let nodes = self.read_nodes(root, cameras.len(), lights.len(), control)?;
        let roots = self.read_scene(root, nodes.len())?;
        let max_depth = u64::from(self.limits.nesting_depth);
        check_hierarchy(&nodes, &roots, max_depth).map_err(|fault| match fault {
            HierarchyFault::Reference => reference(),
            HierarchyFault::Depth(depth) => limit("nesting_depth", depth),
        })?;
        Ok(MeshScene {
            meshes: std::mem::take(&mut self.parts),
            materials: std::mem::take(&mut self.materials),
            mesh_groups: std::mem::take(&mut self.groups),
            nodes,
            roots,
            cameras,
            lights,
            preserved: document,
        })
    }

    fn read_buffers(
        &mut self,
        root: &'a Map<String, Value>,
        resolver: &impl Fn(&str) -> Option<Vec<u8>>,
        control: &mut Control<'_>,
    ) -> Result<()> {
        for (index, value) in array_of(root, "buffers")?.iter().enumerate() {
            self.record()?;
            control.step(len_u64(index), 0)?;
            let obj = object(value)?;
            let length = usize::try_from(
                obj.get("byteLength")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?,
            )
            .map_err(|_| limit("decompressed_bytes", u64::MAX))?;
            self.extensions_of(obj, "buffer", &[], false)?;
            let data: Cow<'a, [u8]> = match obj.get("uri").map(Value::as_str) {
                None => match (index, self.bin) {
                    (0, Some(bin)) => Cow::Borrowed(bin),
                    _ => return Err(invalid()),
                },
                Some(uri) => {
                    let uri = uri.ok_or_else(invalid)?;
                    if let Some(payload) = uri.strip_prefix("data:") {
                        // Bounded by the decoder's remaining bytes, not by `string_bytes`.
                        let remaining = self
                            .limits
                            .decompressed_bytes
                            .saturating_sub(self.owned_bytes);
                        Cow::Owned(decode_data_uri(payload, remaining)?)
                    } else {
                        check_text(uri, &self.limits)?;
                        let bytes = resolver(uri).ok_or_else(|| {
                            Error::new(codes::MESH_RESOURCE_UNAVAILABLE)
                                .with_arg("uri", uri.to_owned())
                        })?;
                        Cow::Owned(bytes)
                    }
                }
            };
            if let Cow::Owned(owned) = &data {
                self.owned_bytes = self.owned_bytes.saturating_add(len_u64(owned.len()));
                if self.owned_bytes > self.limits.decompressed_bytes {
                    return Err(limit("decompressed_bytes", self.owned_bytes));
                }
            }
            if data.len() < length {
                return Err(truncated());
            }
            self.buffers.push(Buffer { data, length });
        }
        Ok(())
    }

    fn read_views(&mut self, root: &Map<String, Value>) -> Result<()> {
        for value in array_of(root, "bufferViews")? {
            self.record()?;
            let obj = object(value)?;
            let buffer = required_index(obj, "buffer")?;
            let declared = self.buffers.get(buffer).ok_or_else(reference)?.length;
            let offset = index_of(obj.get("byteOffset"))?.unwrap_or(0);
            let length = required_index(obj, "byteLength")?;
            let end = offset.checked_add(length).ok_or_else(truncated)?;
            if end > declared {
                return Err(truncated());
            }
            let stride = index_of(obj.get("byteStride"))?;
            self.extensions_of(obj, "bufferView", &[], false)?;
            self.views.push(View {
                buffer,
                offset,
                length,
                stride,
            });
        }
        Ok(())
    }

    fn read_accessors(&mut self, root: &Map<String, Value>) -> Result<()> {
        for value in array_of(root, "accessors")? {
            self.record()?;
            let obj = object(value)?;
            let view = index_of(obj.get("bufferView"))?;
            if let Some(view) = view {
                self.views.get(view).ok_or_else(reference)?;
            }
            let offset = index_of(obj.get("byteOffset"))?.unwrap_or(0);
            let component = Component::from_code(
                obj.get("componentType")
                    .and_then(Value::as_u64)
                    .ok_or_else(invalid)?,
            )?;
            let normalized = flag_of(obj, "normalized")?;
            if normalized && !component.is_integer() {
                return Err(invalid());
            }
            let kind = Kind::parse(
                obj.get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?,
            )?;
            let count = required_index(obj, "count")?;
            self.check_count(count)?;
            let sparse = match obj.get("sparse") {
                None => None,
                Some(sparse) => Some(self.read_sparse(object(sparse)?, count)?),
            };
            // Bounds are checked now, although the data is read only when a mesh uses it.
            let element = kind.element_size(component);
            if let Some(view) = view {
                let stride = self.stride_of(Some(view), element)?;
                self.span(view, offset, stride, count, element)?;
            }
            if let Some(sparse) = sparse {
                let index = sparse.index_component.size();
                self.span(
                    sparse.indices.view,
                    sparse.indices.offset,
                    index,
                    sparse.count,
                    index,
                )?;
                self.span(
                    sparse.values.view,
                    sparse.values.offset,
                    element,
                    sparse.count,
                    element,
                )?;
            }
            self.extensions_of(obj, "accessor", &[], false)?;
            self.accessors.push(Accessor {
                view,
                offset,
                component,
                normalized,
                kind,
                count,
                sparse,
            });
        }
        Ok(())
    }

    fn read_sparse(&self, obj: &Map<String, Value>, count: usize) -> Result<Sparse> {
        let sparse_count = required_index(obj, "count")?;
        if sparse_count > count {
            return Err(invalid());
        }
        let indices = object(obj.get("indices").ok_or_else(invalid)?)?;
        let values = object(obj.get("values").ok_or_else(invalid)?)?;
        let index_component = Component::from_code(
            indices
                .get("componentType")
                .and_then(Value::as_u64)
                .ok_or_else(invalid)?,
        )?;
        if !matches!(
            index_component,
            Component::U8 | Component::U16 | Component::U32
        ) {
            return Err(invalid());
        }
        let span = |part: &Map<String, Value>| -> Result<Span> {
            let view = required_index(part, "bufferView")?;
            self.views.get(view).ok_or_else(reference)?;
            Ok(Span {
                view,
                offset: index_of(part.get("byteOffset"))?.unwrap_or(0),
            })
        };
        Ok(Sparse {
            count: sparse_count,
            indices: span(indices)?,
            index_component,
            values: span(values)?,
        })
    }

    /// Notes the top-level arrays the reader does not interpret.
    fn note_skipped(&mut self, root: &Map<String, Value>) -> Result<()> {
        for key in ["animations", "skins", "samplers"] {
            let count = array_of(root, key)?.len();
            if count > 0 {
                self.loss
                    .add(reasons::ELEMENT_SKIPPED, key, None, len_u64(count));
            }
        }
        let scenes = array_of(root, "scenes")?.len();
        if scenes > 1 {
            self.loss
                .add(reasons::ELEMENT_SKIPPED, "scene", None, len_u64(scenes - 1));
        }
        Ok(())
    }

    fn read_materials(&mut self, root: &Map<String, Value>) -> Result<Vec<Material>> {
        let mut materials: Vec<Material> = Vec::new();
        let mut names = NameSet::default();
        for (index, value) in array_of(root, "materials")?.iter().enumerate() {
            self.record()?;
            let obj = object(value)?;
            let mut material = Material::default();
            let (_, preserved) = self.extensions_of(obj, "material", &[], true)?;
            material.preserved = preserved;
            if let Some(pbr) = obj.get("pbrMetallicRoughness") {
                let pbr = object(pbr)?;
                material.base_color_factor = numbers_of::<4>(pbr, "baseColorFactor")?;
                material.metallic_factor = number_of(pbr, "metallicFactor")?;
                material.roughness_factor = number_of(pbr, "roughnessFactor")?;
                if let Some(texture) = pbr.get("baseColorTexture") {
                    let texture = object(texture)?;
                    material.base_color_texture =
                        Some(self.texture_source(required_index(texture, "index")?, root)?);
                    self.note_tex_coord(texture)?;
                }
                if pbr.contains_key("metallicRoughnessTexture") {
                    self.loss.add(
                        reasons::PROPERTY_SKIPPED,
                        "metallicRoughnessTexture",
                        None,
                        1,
                    );
                }
            }
            for key in [
                "emissiveFactor",
                "emissiveTexture",
                "normalTexture",
                "occlusionTexture",
            ] {
                if obj.contains_key(key) {
                    self.loss.add(reasons::PROPERTY_SKIPPED, key, None, 1);
                }
            }
            material.alpha_mode = match obj.get("alphaMode").map(Value::as_str) {
                None => None,
                Some(Some("OPAQUE")) => Some(AlphaMode::Opaque),
                Some(Some("MASK")) => Some(AlphaMode::Mask),
                Some(Some("BLEND")) => Some(AlphaMode::Blend),
                Some(_) => return Err(value_invalid()),
            };
            material.alpha_cutoff = number_of(obj, "alphaCutoff")?;
            material.double_sided = flag_of(obj, "doubleSided")?;
            material.name =
                names.claim(obj.get("name").and_then(Value::as_str), index, &self.limits)?;
            materials.push(material);
        }
        Ok(materials)
    }

    fn note_tex_coord(&mut self, texture: &Map<String, Value>) -> Result<()> {
        if index_of(texture.get("texCoord"))?.unwrap_or(0) != 0 {
            self.loss
                .add(reasons::PROPERTY_SKIPPED, "texCoord", None, 1);
        }
        Ok(())
    }

    /// The image of a texture: its URI, or the buffer view that holds it.
    fn texture_source(&self, texture: usize, root: &Map<String, Value>) -> Result<ImageSource> {
        let texture = object(
            array_of(root, "textures")?
                .get(texture)
                .ok_or_else(reference)?,
        )?;
        let source = required_index(texture, "source")?;
        let image = object(
            array_of(root, "images")?
                .get(source)
                .ok_or_else(reference)?,
        )?;
        if let Some(uri) = image.get("uri") {
            let uri = uri.as_str().ok_or_else(invalid)?;
            check_text(uri, &self.limits)?;
            return Ok(ImageSource::Uri(uri.to_owned()));
        }
        let view = required_index(image, "bufferView")?;
        self.views.get(view).ok_or_else(reference)?;
        Ok(ImageSource::BufferView(view))
    }

    fn read_mesh(
        &mut self,
        obj: &Map<String, Value>,
        control: &mut Control<'_>,
    ) -> Result<MeshGroup> {
        self.record()?;
        let name = text_of(obj, "name", &self.limits)?;
        let (_, preserved) = self.extensions_of(obj, "mesh", &[], true)?;
        let name_bytes = len_u64(name.len());
        let mut primitives = Vec::new();
        for value in array_of(obj, "primitives")? {
            if let Some(part) = self.read_primitive(object(value)?, name_bytes, control)? {
                self.parts.push(part);
                primitives.push(self.parts.len() - 1);
            }
        }
        Ok(MeshGroup {
            name,
            primitives,
            preserved,
        })
    }

    /// Reads one primitive. Its size is worked out from the accessors and charged against
    /// `Limits::decompressed_bytes` before any of its data is decoded, so a zero-filled accessor
    /// shared by many primitives cannot exceed the limit. `group_name_bytes` is the length of the
    /// name of the mesh that holds the primitive.
    fn read_primitive(
        &mut self,
        obj: &Map<String, Value>,
        group_name_bytes: u64,
        control: &mut Control<'_>,
    ) -> Result<Option<MeshPart>> {
        self.record()?;
        control.step(self.decoded, self.limits.decompressed_bytes)?;
        let mode = index_of(obj.get("mode"))?.map_or(MODE_TRIANGLES, len_u64);
        if !matches!(mode, MODE_TRIANGLES | MODE_STRIP | MODE_FAN) {
            self.loss
                .add(reasons::ELEMENT_SKIPPED, "primitive mode", None, 1);
            return Ok(None);
        }
        let (_, preserved) = self.extensions_of(obj, "primitive", &[], true)?;
        let attributes = object(obj.get("attributes").ok_or_else(invalid)?)?;
        for key in attributes.keys() {
            if !matches!(
                key.as_str(),
                "POSITION" | "NORMAL" | "TEXCOORD_0" | "COLOR_0"
            ) {
                self.loss.add(reasons::PROPERTY_SKIPPED, key, None, 1);
            }
        }
        let position = self
            .attribute(attributes, "POSITION", &[Kind::Vec3])?
            .ok_or_else(invalid)?;
        let vertices = self.accessor(position)?.count;
        let normal = self.attribute(attributes, "NORMAL", &[Kind::Vec3])?;
        let uv = self.attribute(attributes, "TEXCOORD_0", &[Kind::Vec2])?;
        let color = self.attribute(attributes, "COLOR_0", &[Kind::Vec3, Kind::Vec4])?;
        for attribute in [normal, uv, color].into_iter().flatten() {
            if self.accessor(attribute)?.count != vertices {
                return Err(invalid());
            }
        }
        let index_accessor = match obj.get("indices") {
            Some(index) => Some(index_of(Some(index))?.ok_or_else(invalid)?),
            None => None,
        };
        let index_count = match index_accessor {
            Some(index) => self.index_accessor_count(index)?,
            None => {
                u32::try_from(vertices).map_err(|_| limit("array_length", len_u64(vertices)))?;
                vertices
            }
        };
        let triangle_count = match mode {
            MODE_TRIANGLES => index_count / 3,
            _ => index_count.saturating_sub(2),
        };
        let material = match index_of(obj.get("material"))? {
            None => None,
            Some(index) => Some(
                self.materials
                    .get(index)
                    .ok_or_else(reference)?
                    .name
                    .clone(),
            ),
        };
        let names = group_name_bytes
            .saturating_add(material.as_ref().map_or(0, |name| len_u64(name.len())));
        let bytes = primitive_bytes(
            vertices,
            [normal.is_some(), uv.is_some(), color.is_some()],
            index_count,
            triangle_count,
            names,
        )
        .ok_or_else(|| limit("decompressed_bytes", u64::MAX))?;
        self.charge_decoded(bytes)?;

        let position = self.accessor_floats(position, control)?;
        let normal = normal
            .map(|index| self.accessor_floats(index, control))
            .transpose()?;
        let uv = uv
            .map(|index| self.accessor_floats(index, control))
            .transpose()?;
        let color = color
            .map(|index| self.accessor_floats(index, control))
            .transpose()?;
        let indices = match index_accessor {
            Some(index) => self.index_accessor(index, control)?,
            None => {
                let top = u32::try_from(vertices)
                    .map_err(|_| limit("array_length", len_u64(vertices)))?;
                (0..top).collect()
            }
        };
        let triangles = triangles_of(mode, &indices, vertices)?;
        let channels = channels_of(normal.as_ref(), uv.as_ref(), color.as_ref())?;
        let positions = vectors::<3>(&position.values)?
            .into_iter()
            .map(|[x, y, z]| Point3::new(f64::from(x), f64::from(y), f64::from(z)))
            .collect();
        let mesh = build_mesh(positions, triangles, channels, &self.limits)?;
        Ok(Some(MeshPart {
            name: String::new(),
            mesh,
            material,
            preserved,
        }))
    }

    /// The accessor index of a named attribute, or `None` when absent. Its kind must be one of
    /// `kinds`; its count is checked against the positions by the caller.
    fn attribute(
        &self,
        attributes: &Map<String, Value>,
        name: &str,
        kinds: &[Kind],
    ) -> Result<Option<usize>> {
        let Some(index) = attributes.get(name) else {
            return Ok(None);
        };
        let index = index_of(Some(index))?.ok_or_else(invalid)?;
        if !kinds.contains(&self.accessor(index)?.kind) {
            return Err(invalid());
        }
        Ok(Some(index))
    }

    /// The count of an index accessor, which must be a scalar of an unsigned component type.
    fn index_accessor_count(&self, index: usize) -> Result<usize> {
        let accessor = self.accessor(index)?;
        let unsigned = matches!(
            accessor.component,
            Component::U8 | Component::U16 | Component::U32
        );
        if accessor.kind != Kind::Scalar || !unsigned {
            return Err(invalid());
        }
        Ok(accessor.count)
    }
}

/// The floats read from an accessor: its elements of `components` values each.
struct Floats {
    components: usize,
    values: Vec<f32>,
}

/// The attribute channels of a primitive: normals, the first texture coordinates and colors. A
/// color of three components gets alpha 1.
fn channels_of(
    normal: Option<&Floats>,
    uv: Option<&Floats>,
    color: Option<&Floats>,
) -> Result<Vec<(AttributeKey, Channel)>> {
    let mut channels = Vec::new();
    if let Some(values) = normal {
        channels.push((
            AttributeKey::Normal,
            Channel::Vec3(vectors::<3>(&values.values)?),
        ));
    }
    if let Some(values) = uv {
        channels.push((
            AttributeKey::Uv(0),
            Channel::Vec2(vectors::<2>(&values.values)?),
        ));
    }
    if let Some(values) = color {
        let colors = if values.components == 3 {
            vectors::<3>(&values.values)?
                .into_iter()
                .map(|[r, g, b]| [r, g, b, 1.0])
                .collect()
        } else {
            vectors::<4>(&values.values)?
        };
        channels.push((AttributeKey::Color, Channel::Vec4(colors)));
    }
    Ok(channels)
}

/// The bytes one primitive holds once decoded, or `None` on overflow: positions (24 bytes per
/// vertex, `f64`), the channels that are present (normals 12 bytes, texture coordinates 8 and
/// colors 16, stored as `Vec4`), 4 bytes per index, 12 bytes per triangle and the names.
fn primitive_bytes(
    vertices: usize,
    channels: [bool; 3],
    indices: usize,
    triangles: usize,
    names: u64,
) -> Option<u64> {
    let [normal, uv, color] = channels.map(u64::from);
    let per_vertex = 24 + 12 * normal + 8 * uv + 16 * color;
    len_u64(vertices)
        .checked_mul(per_vertex)?
        .checked_add(len_u64(indices).checked_mul(4)?)?
        .checked_add(len_u64(triangles).checked_mul(12)?)?
        .checked_add(names)
}

/// The vertex attributes and index lists of the `read_primitive` call.
impl Reader<'_> {
    fn accessor(&self, index: usize) -> Result<&Accessor> {
        self.accessors.get(index).ok_or_else(reference)
    }

    fn view(&self, index: usize) -> Result<View> {
        self.views.get(index).copied().ok_or_else(reference)
    }

    fn buffer(&self, index: usize) -> Result<&[u8]> {
        let buffer = self.buffers.get(index).ok_or_else(reference)?;
        Ok(buffer.data.get(..buffer.length).unwrap_or_default())
    }

    /// The stride of an accessor's elements: the view's stride, or the element size when the view
    /// gives none. A stride shorter than an element is refused.
    fn stride_of(&self, view: Option<usize>, element: usize) -> Result<usize> {
        match view {
            None => Ok(element),
            Some(view) => match self.view(view)?.stride {
                None => Ok(element),
                Some(stride) if stride < element => Err(invalid()),
                Some(stride) => Ok(stride),
            },
        }
    }

    /// Checks that `count` elements of `element` bytes, `stride` bytes apart and starting `start`
    /// bytes into the view, lie inside the view. Returns the buffer and the offset of the first
    /// element in it. Nothing is read before this check, so a declared count cannot drive an
    /// allocation beyond the data.
    fn span(
        &self,
        view: usize,
        start: usize,
        stride: usize,
        count: usize,
        element: usize,
    ) -> Result<(usize, usize)> {
        let view = self.view(view)?;
        if count > 0 {
            let last = count
                .checked_sub(1)
                .and_then(|last| last.checked_mul(stride))
                .and_then(|at| at.checked_add(start))
                .and_then(|at| at.checked_add(element))
                .ok_or_else(truncated)?;
            if last > view.length {
                return Err(truncated());
            }
        }
        let first = view.offset.checked_add(start).ok_or_else(truncated)?;
        Ok((view.buffer, first))
    }

    /// Reads `count` elements of `components` values each from a view, converting to `f32`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the view span and the component layout are read together"
    )]
    fn gather(
        &self,
        view: usize,
        start: usize,
        stride: usize,
        count: usize,
        component: Component,
        normalized: bool,
        components: usize,
        control: &mut Control<'_>,
    ) -> Result<Vec<f32>> {
        if components == 0 {
            return Err(invalid());
        }
        let size = component.size();
        let element = components.checked_mul(size).ok_or_else(truncated)?;
        let (buffer, first) = self.span(view, start, stride, count, element)?;
        let data = self.buffer(buffer)?;
        let total = count.checked_mul(components).ok_or_else(truncated)?;
        let mut out = vec![0.0_f32; total];
        for (index, element_out) in out.chunks_exact_mut(components).enumerate() {
            if index.is_multiple_of(GATHER_STEP) {
                control.step(len_u64(index), len_u64(count))?;
            }
            let at = index
                .checked_mul(stride)
                .and_then(|offset| first.checked_add(offset))
                .ok_or_else(truncated)?;
            for (channel, slot) in element_out.iter_mut().enumerate() {
                let position = channel
                    .checked_mul(size)
                    .and_then(|offset| at.checked_add(offset))
                    .ok_or_else(truncated)?;
                *slot = read_component(data, position, component, normalized)?;
            }
        }
        Ok(out)
    }

    /// Reads the indices of a scalar accessor of `component` type.
    fn gather_indices(
        &self,
        view: usize,
        start: usize,
        stride: usize,
        count: usize,
        component: Component,
        control: &mut Control<'_>,
    ) -> Result<Vec<u32>> {
        let (buffer, first) = self.span(view, start, stride, count, component.size())?;
        let data = self.buffer(buffer)?;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            if index.is_multiple_of(GATHER_STEP) {
                control.step(len_u64(index), len_u64(count))?;
            }
            let at = index
                .checked_mul(stride)
                .and_then(|offset| first.checked_add(offset))
                .ok_or_else(truncated)?;
            out.push(read_index(data, at, component)?);
        }
        Ok(out)
    }

    /// The floats of an accessor, with its sparse part applied. Matrix accessors are not read.
    fn accessor_floats(&self, index: usize, control: &mut Control<'_>) -> Result<Floats> {
        let accessor = *self.accessor(index)?;
        if accessor.kind.is_matrix() {
            return Err(invalid());
        }
        let components = accessor.kind.components();
        self.check_count(accessor.count)?;
        let mut values = match accessor.view {
            Some(view) => {
                let stride =
                    self.stride_of(Some(view), accessor.kind.element_size(accessor.component))?;
                self.gather(
                    view,
                    accessor.offset,
                    stride,
                    accessor.count,
                    accessor.component,
                    accessor.normalized,
                    components,
                    control,
                )?
            }
            None => {
                let total = accessor
                    .count
                    .checked_mul(components)
                    .ok_or_else(truncated)?;
                vec![0.0_f32; total]
            }
        };
        if let Some(sparse) = accessor.sparse {
            let size = accessor.component.size();
            let element = components.checked_mul(size).ok_or_else(truncated)?;
            let targets = self.gather_indices(
                sparse.indices.view,
                sparse.indices.offset,
                sparse.index_component.size(),
                sparse.count,
                sparse.index_component,
                control,
            )?;
            let replacement = self.gather(
                sparse.values.view,
                sparse.values.offset,
                element,
                sparse.count,
                accessor.component,
                accessor.normalized,
                components,
                control,
            )?;
            for (target, value) in targets.iter().zip(replacement.chunks_exact(components)) {
                let target = usize::try_from(*target).map_err(|_| reference())?;
                if target >= accessor.count {
                    return Err(reference());
                }
                let at = target.checked_mul(components).ok_or_else(truncated)?;
                let slot = values
                    .get_mut(at..at.checked_add(components).ok_or_else(truncated)?)
                    .ok_or_else(truncated)?;
                for (destination, source) in slot.iter_mut().zip(value) {
                    *destination = *source;
                }
            }
        }
        Ok(Floats { components, values })
    }

    /// The indices of a scalar accessor of an unsigned component type, with sparse data applied.
    fn index_accessor(&self, index: usize, control: &mut Control<'_>) -> Result<Vec<u32>> {
        let accessor = *self.accessor(index)?;
        self.index_accessor_count(index)?;
        self.check_count(accessor.count)?;
        let mut values = match accessor.view {
            Some(view) => {
                let stride = self.stride_of(Some(view), accessor.component.size())?;
                self.gather_indices(
                    view,
                    accessor.offset,
                    stride,
                    accessor.count,
                    accessor.component,
                    control,
                )?
            }
            None => vec![0; accessor.count],
        };
        if let Some(sparse) = accessor.sparse {
            let targets = self.gather_indices(
                sparse.indices.view,
                sparse.indices.offset,
                sparse.index_component.size(),
                sparse.count,
                sparse.index_component,
                control,
            )?;
            let replacement = self.gather_indices(
                sparse.values.view,
                sparse.values.offset,
                accessor.component.size(),
                sparse.count,
                accessor.component,
                control,
            )?;
            for (target, value) in targets.iter().zip(replacement) {
                let target = usize::try_from(*target).map_err(|_| reference())?;
                *values.get_mut(target).ok_or_else(reference)? = value;
            }
        }
        Ok(values)
    }

    fn read_nodes(
        &mut self,
        root: &Map<String, Value>,
        camera_count: usize,
        light_count: usize,
        control: &mut Control<'_>,
    ) -> Result<Vec<Node>> {
        let values = array_of(root, "nodes")?;
        let mut nodes = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            self.record()?;
            control.step(len_u64(index), len_u64(values.len()))?;
            let obj = object(value)?;
            let (taken, preserved) = self.extensions_of(obj, "node", &[LIGHTS], true)?;
            let transform = read_transform(obj)?;
            let mesh = index_of(obj.get("mesh"))?;
            if let Some(mesh) = mesh
                && mesh >= self.groups.len()
            {
                return Err(reference());
            }
            let camera = index_of(obj.get("camera"))?;
            if let Some(camera) = camera
                && camera >= camera_count
            {
                return Err(reference());
            }
            let light = match taken.get(LIGHTS) {
                None => None,
                Some(light) => {
                    let light = required_index(object(light)?, "light")?;
                    if light >= light_count {
                        return Err(reference());
                    }
                    Some(light)
                }
            };
            let mut children = Vec::new();
            for child in array_of(obj, "children")? {
                let child = index_of(Some(child))?.ok_or_else(invalid)?;
                if child >= values.len() {
                    return Err(reference());
                }
                children.push(child);
            }
            nodes.push(Node {
                name: text_of(obj, "name", &self.limits)?,
                transform,
                mesh,
                children,
                camera,
                light,
                preserved,
            });
        }
        Ok(nodes)
    }

    fn read_cameras(&mut self, root: &Map<String, Value>) -> Result<Vec<Camera>> {
        let mut cameras = Vec::new();
        for value in array_of(root, "cameras")? {
            self.record()?;
            let obj = object(value)?;
            let (_, preserved) = self.extensions_of(obj, "camera", &[], true)?;
            let projection = match obj.get("type").and_then(Value::as_str) {
                Some("perspective") => {
                    let body = object(obj.get("perspective").ok_or_else(invalid)?)?;
                    Projection::Perspective {
                        yfov: number_of(body, "yfov")?.ok_or_else(invalid)?,
                        aspect_ratio: number_of(body, "aspectRatio")?,
                        znear: number_of(body, "znear")?.ok_or_else(invalid)?,
                        zfar: number_of(body, "zfar")?,
                    }
                }
                Some("orthographic") => {
                    let body = object(obj.get("orthographic").ok_or_else(invalid)?)?;
                    Projection::Orthographic {
                        xmag: number_of(body, "xmag")?.ok_or_else(invalid)?,
                        ymag: number_of(body, "ymag")?.ok_or_else(invalid)?,
                        znear: number_of(body, "znear")?.ok_or_else(invalid)?,
                        zfar: number_of(body, "zfar")?.ok_or_else(invalid)?,
                    }
                }
                _ => return Err(invalid()),
            };
            cameras.push(Camera {
                name: text_of(obj, "name", &self.limits)?,
                projection,
                preserved,
            });
        }
        Ok(cameras)
    }

    fn read_lights(&mut self, root_extensions: &Map<String, Value>) -> Result<Vec<Light>> {
        let Some(extension) = root_extensions.get(LIGHTS) else {
            return Ok(Vec::new());
        };
        let extension = object(extension)?;
        let mut lights = Vec::new();
        for value in array_of(extension, "lights")? {
            self.record()?;
            let obj = object(value)?;
            let (_, preserved) = self.extensions_of(obj, "light", &[], true)?;
            let kind = match obj.get("type").and_then(Value::as_str) {
                Some("directional") => LightKind::Directional,
                Some("point") => LightKind::Point,
                Some("spot") => {
                    let spot = match obj.get("spot") {
                        None => None,
                        Some(spot) => Some(object(spot)?),
                    };
                    LightKind::Spot {
                        inner_cone_angle: spot
                            .map(|s| number_of(s, "innerConeAngle"))
                            .transpose()?
                            .flatten()
                            .unwrap_or(0.0),
                        outer_cone_angle: spot
                            .map(|s| number_of(s, "outerConeAngle"))
                            .transpose()?
                            .flatten()
                            .unwrap_or(FRAC_PI_4),
                    }
                }
                _ => return Err(invalid()),
            };
            lights.push(Light {
                name: text_of(obj, "name", &self.limits)?,
                kind,
                color: numbers_of::<3>(obj, "color")?.unwrap_or([1.0; 3]),
                intensity: number_of(obj, "intensity")?.unwrap_or(1.0),
                range: number_of(obj, "range")?,
                preserved,
            });
        }
        Ok(lights)
    }

    /// The root nodes of the default scene: the scene named by `scene`, or the first one.
    fn read_scene(&mut self, root: &Map<String, Value>, node_count: usize) -> Result<Vec<usize>> {
        let scenes = array_of(root, "scenes")?;
        let chosen = index_of(root.get("scene"))?.unwrap_or(0);
        let Some(scene) = scenes.get(chosen) else {
            return if scenes.is_empty() {
                Ok(Vec::new())
            } else {
                Err(reference())
            };
        };
        let mut roots = Vec::new();
        for node in array_of(object(scene)?, "nodes")? {
            let node = index_of(Some(node))?.ok_or_else(invalid)?;
            if node >= node_count {
                return Err(reference());
            }
            roots.push(node);
        }
        Ok(roots)
    }
}

/// Reads one component at `at`, converted to `f32`.
fn read_component(data: &[u8], at: usize, component: Component, normalized: bool) -> Result<f32> {
    let end = at.checked_add(component.size()).ok_or_else(truncated)?;
    let raw = data.get(at..end).ok_or_else(truncated)?;
    let value = match component {
        Component::I8 => {
            let value = f64::from(i8::from_le_bytes(array(raw)?));
            if normalized {
                (value / 127.0).max(-1.0)
            } else {
                value
            }
        }
        Component::U8 => {
            let value = f64::from(u8::from_le_bytes(array(raw)?));
            if normalized { value / 255.0 } else { value }
        }
        Component::I16 => {
            let value = f64::from(i16::from_le_bytes(array(raw)?));
            if normalized {
                (value / 32767.0).max(-1.0)
            } else {
                value
            }
        }
        Component::U16 => {
            let value = f64::from(u16::from_le_bytes(array(raw)?));
            if normalized { value / 65535.0 } else { value }
        }
        Component::U32 => f64::from(u32::from_le_bytes(array(raw)?)),
        Component::F32 => {
            let value = f32::from_le_bytes(array(raw)?);
            if !value.is_finite() {
                return Err(value_invalid());
            }
            f64::from(value)
        }
    };
    Ok(narrow(value))
}

/// Reads one index of an unsigned component type.
fn read_index(data: &[u8], at: usize, component: Component) -> Result<u32> {
    let end = at.checked_add(component.size()).ok_or_else(truncated)?;
    let raw = data.get(at..end).ok_or_else(truncated)?;
    match component {
        Component::U8 => Ok(u32::from(u8::from_le_bytes(array(raw)?))),
        Component::U16 => Ok(u32::from(u16::from_le_bytes(array(raw)?))),
        Component::U32 => Ok(u32::from_le_bytes(array(raw)?)),
        _ => Err(invalid()),
    }
}

/// Groups flat values into fixed-size vectors.
fn vectors<const N: usize>(values: &[f32]) -> Result<Vec<[f32; N]>> {
    values
        .chunks_exact(N)
        .map(|chunk| <[f32; N]>::try_from(chunk).map_err(|_| invalid()))
        .collect()
}

/// The triangles of a primitive from its indices: a list, a strip or a fan.
fn triangles_of(mode: u64, indices: &[u32], vertices: usize) -> Result<Vec<[u32; 3]>> {
    let triangles: Vec<[u32; 3]> = match mode {
        MODE_TRIANGLES => {
            if !indices.len().is_multiple_of(3) {
                return Err(invalid());
            }
            indices
                .chunks_exact(3)
                .map(|c| <[u32; 3]>::try_from(c).map_err(|_| invalid()))
                .collect::<Result<_>>()?
        }
        MODE_STRIP | MODE_FAN => {
            if indices.len() < 3 {
                return Err(Error::new(codes::MESH_FACE_INVALID));
            }
            let mut out = Vec::with_capacity(indices.len() - 2);
            for (k, window) in indices.windows(3).enumerate() {
                let [a, b, c] = <[u32; 3]>::try_from(window).map_err(|_| invalid())?;
                out.push(if mode == MODE_FAN {
                    [indices.first().copied().ok_or_else(invalid)?, b, c]
                } else if k % 2 == 0 {
                    [a, b, c]
                } else {
                    [b, a, c]
                });
            }
            out
        }
        _ => return Err(invalid()),
    };
    let count = u32::try_from(vertices).map_err(|_| invalid())?;
    if triangles.iter().flatten().any(|&index| index >= count) {
        return Err(Error::new(codes::MESH_INDEX_OUT_OF_RANGE));
    }
    Ok(triangles)
}

/// The transform of a node: a matrix, or the translation, rotation and scale it gives.
fn read_transform(obj: &Map<String, Value>) -> Result<Transform> {
    let trs = obj.contains_key("translation")
        || obj.contains_key("rotation")
        || obj.contains_key("scale");
    match (obj.get("matrix"), trs) {
        (Some(_), true) => Err(invalid()),
        (Some(matrix), false) => Ok(Transform::Matrix(numbers::<16>(matrix)?)),
        (None, _) => Ok(Transform::Trs {
            translation: numbers_of::<3>(obj, "translation")?.unwrap_or([0.0; 3]),
            rotation: numbers_of::<4>(obj, "rotation")?.unwrap_or([0.0, 0.0, 0.0, 1.0]),
            scale: numbers_of::<3>(obj, "scale")?.unwrap_or([1.0; 3]),
        }),
    }
}

/// The material names taken so far. A name is kept when the file gives one that is unused;
/// otherwise the name is `material-<index>`, and `#<n>` is appended while that is taken. The
/// next suffix of each base is remembered, so repeated collisions stay linear.
#[derive(Default)]
struct NameSet {
    taken: HashSet<String>,
    next_suffix: HashMap<String, u64>,
}

impl NameSet {
    fn claim(&mut self, name: Option<&str>, index: usize, limits: &Limits) -> Result<String> {
        if let Some(name) = name {
            check_text(name, limits)?;
            if self.taken.insert(name.to_owned()) {
                return Ok(name.to_owned());
            }
        }
        let base = format!("material-{index}");
        let mut candidate = base.clone();
        let mut suffix = self.next_suffix.get(&base).copied().unwrap_or(1);
        while self.taken.contains(&candidate) {
            candidate = format!("{base}#{suffix}");
            suffix += 1;
        }
        self.next_suffix.insert(base, suffix);
        self.taken.insert(candidate.clone());
        Ok(candidate)
    }
}

/// Decodes the payload of a `data:` URI (the text after `data:`), which must be base64.
fn decode_data_uri(payload: &str, remaining: u64) -> Result<Vec<u8>> {
    let (head, data) = payload.split_once(',').ok_or_else(value_invalid)?;
    if !head.ends_with(";base64") {
        return Err(value_invalid());
    }
    base64_decode(data.as_bytes(), remaining)
}

fn base64_value(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Decodes standard base64 with optional padding. The output may not exceed `max` bytes.
///
/// Padding is accepted only at the end of a complete quantum, and only as many `=` as the data
/// leaves open: none after a whole quantum, one after three characters and two after two.
fn base64_decode(input: &[u8], max: u64) -> Result<Vec<u8>> {
    let pad = input.iter().rev().take_while(|&&byte| byte == b'=').count();
    let data = input
        .get(..input.len().saturating_sub(pad))
        .unwrap_or_default();
    let open = (4 - data.len() % 4) % 4;
    let misplaced = pad > 0 && (!input.len().is_multiple_of(4) || pad != open);
    if pad > 2 || data.contains(&b'=') || misplaced || data.len() % 4 == 1 {
        return Err(value_invalid());
    }
    // The reserve is bounded by the output that may be produced, and by the input it comes from.
    let allowed = usize::try_from(max).unwrap_or(usize::MAX);
    let reserve = (data.len() / 4 * 3 + 2).min(input.len()).min(allowed);
    let mut out = Vec::with_capacity(reserve);
    let mut bits: u32 = 0;
    let mut accumulator: u32 = 0;
    for &byte in data {
        let value = base64_value(byte).ok_or_else(value_invalid)?;
        accumulator = ((accumulator << 6) | value) & 0xFFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((accumulator >> bits) & 0xFF).unwrap_or_default());
            if len_u64(out.len()) > max {
                return Err(limit("decompressed_bytes", len_u64(out.len())));
            }
        }
    }
    Ok(out)
}

/// The base64 encoding of bytes, with padding.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let group = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for shift in [18, 12, 6, 0] {
            let index = usize::try_from((group >> shift) & 0x3F).unwrap_or_default();
            out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'=')));
        }
        if chunk.len() < 3 {
            let padding = 3 - chunk.len();
            let start = out.len() - padding;
            out.truncate(start);
            out.push_str(&"=".repeat(padding));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Component, Kind, base64_decode, base64_encode, check_nesting};

    #[test]
    fn matrix_columns_are_padded_to_four_bytes() {
        assert_eq!(Kind::Mat2.element_size(Component::U8), 8);
        assert_eq!(Kind::Mat3.element_size(Component::U8), 12);
        assert_eq!(Kind::Mat3.element_size(Component::U16), 24);
        assert_eq!(Kind::Mat4.element_size(Component::F32), 64);
        assert_eq!(Kind::Vec3.element_size(Component::U8), 3);
        assert_eq!(Kind::Vec3.element_size(Component::F32), 12);
    }

    #[test]
    fn base64_encodes_with_padding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn base64_accepts_only_the_padding_its_quantum_needs() {
        let refused: [&[u8]; 7] = [
            b"AAAA====",
            b"====",
            b"AAAA=",
            b"AAA==",
            b"AA===",
            b"AA=",
            b"A===",
        ];
        for text in refused {
            assert!(base64_decode(text, u64::MAX).is_err(), "{text:?}");
        }
        for text in [&b""[..], b"AAAA", b"AAA=", b"AA=="] {
            assert!(base64_decode(text, u64::MAX).is_ok(), "{text:?}");
        }
    }

    #[test]
    fn base64_reserve_is_capped_by_the_allowed_output() {
        let out = base64_decode(b"AAAA", 3).unwrap();
        assert_eq!(out, vec![0, 0, 0]);
        assert!(out.capacity() <= 3);
    }

    #[test]
    fn nesting_ignores_brackets_inside_strings() {
        assert!(check_nesting(br#"{"a":"[[[[[[[[[["}"#, 2).is_ok());
        assert!(check_nesting(br#"[[[]]]"#, 2).is_err());
        assert!(check_nesting(br#"{"s":"\"[[[[","t":[[1]]}"#, 3).is_ok());
    }
}
