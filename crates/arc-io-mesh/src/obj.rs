//! Wavefront OBJ with its MTL material library (CORE-IO-025).
//!
//! Reading takes the OBJ bytes and, optionally, the MTL bytes; the crate never opens files, so
//! the `mtllib` statement is ignored and the caller supplies the library. Vertices (`v`), texture
//! coordinates (`vt`) and normals (`vn`) take the one-based or negative (relative) indices of the
//! file. Each `o` or `g` statement starts a part; a `usemtl` statement after faces starts a part
//! too, so every part has one material. A part is a vertex per distinct corner (position, texture
//! coordinate and normal together), which keeps attributes per vertex. Polygons are fan
//! triangulated. A new object or group starts without a material, so writing always names the
//! material of each part after its `o` statement. Smoothing groups (`s`) and the `mtllib` statement
//! are not kept. Continuation lines (a backslash at the end) are joined; comments are removed.
//!
//! Writing emits the parts in order, with every vertex written once, and the MTL bytes for the
//! materials of the scene. Vertex colors and channels other than normals and the first texture
//! coordinate set are dropped and reported.

use std::collections::HashMap;

use arc_base::math::Point3;
use arc_base::{Error, Limits, Result};
use arc_mesh::{AttributeKey, Channel};

use crate::error::{at_line, codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog};
use crate::model::{Material, MeshPart, MeshScene, TextureMap, attribute_name, build_mesh};
use crate::options::{Control, Encoding, ReadOptions, ReadOutput, WriteOptions, WriteOutput};
use crate::text::{self, Lines, len_u64, text32, text64};

/// Reads an OBJ file and the MTL library it names, if the caller supplies it.
///
/// # Errors
///
/// `io.limit_exceeded` naming the limit, `io.mesh.index_out_of_range` for a face that refers to a
/// missing vertex, `io.mesh.face_invalid`, `io.mesh.structure_invalid`, `io.mesh.value_invalid`,
/// `io.mesh.text_invalid` and `base.cancelled`.
pub fn read(obj: &[u8], mtl: Option<&[u8]>, options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits = options.limits;
    text::check_input(obj.len(), &limits)?;
    let mut control = options.control();
    let mut loss = LossLog::default();
    let materials = match mtl {
        Some(bytes) => {
            text::check_input(bytes.len(), &limits)?;
            read_mtl(bytes, &limits, &mut control, &mut loss)?
        }
        None => Vec::new(),
    };
    let mut reader = Reader::new(limits, loss);
    reader.run(obj, &mut control)?;
    let (meshes, loss) = reader.finish()?;
    control.finish(len_u64(obj.len()));
    let report = loss.finish(LossHeader::new("obj", "", Direction::Import, Some(limits)));
    Ok(ReadOutput {
        scene: MeshScene { meshes, materials },
        report,
    })
}

/// The corner of a face: a position and, optionally, a texture coordinate and a normal, as
/// indices into the file's pools.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Corner {
    position: u32,
    uv: Option<u32>,
    normal: Option<u32>,
}

/// A part under construction: its vertices (one per distinct corner) and triangles.
#[derive(Default)]
struct Part {
    name: String,
    material: Option<String>,
    vertices: Vec<Corner>,
    index: HashMap<Corner, u32>,
    triangles: Vec<[u32; 3]>,
}

impl Part {
    fn named(name: String, material: Option<String>) -> Self {
        Self {
            name,
            material,
            ..Self::default()
        }
    }

    fn has_faces(&self) -> bool {
        !self.triangles.is_empty()
    }

    fn vertex(&mut self, corner: Corner, limits: &Limits) -> Result<u32> {
        if let Some(&index) = self.index.get(&corner) {
            return Ok(index);
        }
        let count = len_u64(self.vertices.len());
        let index = u32::try_from(count)
            .ok()
            .filter(|_| count < limits.array_length)
            .ok_or_else(|| limit("array_length", count + 1))?;
        self.vertices.push(corner);
        self.index.insert(corner, index);
        Ok(index)
    }
}

/// The statement state of a read: the pools of the file, the parts and the loss log.
struct Reader {
    limits: Limits,
    positions: Vec<Point3>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    records: u64,
    finished: Vec<Part>,
    current: Part,
    loss: LossLog,
}

impl Reader {
    fn new(limits: Limits, loss: LossLog) -> Self {
        Self {
            limits,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            records: 0,
            finished: Vec::new(),
            current: Part::default(),
            loss,
        }
    }

    fn run(&mut self, data: &[u8], control: &mut Control<'_>) -> Result<()> {
        let total = len_u64(data.len());
        let mut lines = Lines::new(data, &self.limits);
        let mut statement = String::new();
        while let Some(item) = lines.next() {
            let (number, raw) = item?;
            control.step(lines.offset(), total)?;
            let line = text::text(number, raw)?;
            let body = line.split('#').next().unwrap_or_default();
            let (body, continued) = match body.trim_end().strip_suffix('\\') {
                Some(stripped) => (stripped, true),
                None => (body, false),
            };
            statement.push_str(body);
            statement.push(' ');
            if len_u64(statement.len()) > self.limits.string_bytes {
                return Err(limit("string_bytes", len_u64(statement.len())));
            }
            if !continued {
                let complete = std::mem::take(&mut statement);
                self.statement(number, &complete)?;
            }
        }
        if !statement.is_empty() {
            self.statement(lines.number(), &statement)?;
        }
        Ok(())
    }

    fn count_record(&mut self) -> Result<()> {
        self.records = self.records.saturating_add(1);
        if self.records > self.limits.record_count {
            return Err(limit("record_count", self.records));
        }
        Ok(())
    }

    fn statement(&mut self, number: u64, statement: &str) -> Result<()> {
        let (keyword, rest) = split_keyword(statement);
        match keyword {
            "" => Ok(()),
            "v" => self.vertex(number, rest),
            "vt" => self.texture(number, rest),
            "vn" => self.normal(number, rest),
            "f" => self.face(number, rest),
            "o" | "g" => {
                self.start_part(rest.to_owned());
                Ok(())
            }
            "usemtl" if !rest.is_empty() => {
                self.use_material(rest.to_owned());
                Ok(())
            }
            "usemtl" => Err(at_line(codes::MESH_STRUCTURE_INVALID, number)),
            "mtllib" | "s" => Ok(()),
            other => {
                self.loss
                    .add(reasons::ELEMENT_SKIPPED, other, Some(number), 1);
                Ok(())
            }
        }
    }

    fn vertex(&mut self, number: u64, rest: &str) -> Result<()> {
        self.count_record()?;
        let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
        let [x, y, z, extra @ ..] = fields.as_slice() else {
            return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
        };
        let position = Point3::new(
            text::float64(number, x)?,
            text::float64(number, y)?,
            text::float64(number, z)?,
        );
        match extra {
            [] => {}
            [weight] => {
                text::float64(number, weight)?;
                self.loss
                    .add(reasons::PROPERTY_SKIPPED, "vertex_w", Some(number), 1);
            }
            [r, g, b] => {
                check_numbers(number, &[*r, *g, *b])?;
                self.loss.attribute(reasons::ATTRIBUTE_DROPPED, "color", 1);
            }
            [weight, r, g, b] => {
                check_numbers(number, &[*weight, *r, *g, *b])?;
                self.loss
                    .add(reasons::PROPERTY_SKIPPED, "vertex_w", Some(number), 1);
                self.loss.attribute(reasons::ATTRIBUTE_DROPPED, "color", 1);
            }
            _ => return Err(at_line(codes::MESH_STRUCTURE_INVALID, number)),
        }
        self.positions.push(position);
        Ok(())
    }

    fn texture(&mut self, number: u64, rest: &str) -> Result<()> {
        self.count_record()?;
        let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
        let (u, v, w) = match fields.as_slice() {
            [u] => (*u, "0", None),
            [u, v] => (*u, *v, None),
            [u, v, w] => (*u, *v, Some(*w)),
            _ => return Err(at_line(codes::MESH_STRUCTURE_INVALID, number)),
        };
        if let Some(w) = w {
            text::float32(number, w)?;
            self.loss
                .add(reasons::PROPERTY_SKIPPED, "texture_w", Some(number), 1);
        }
        self.uvs
            .push([text::float32(number, u)?, text::float32(number, v)?]);
        Ok(())
    }

    fn normal(&mut self, number: u64, rest: &str) -> Result<()> {
        self.count_record()?;
        let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
        let [x, y, z] = fields.as_slice() else {
            return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
        };
        self.normals.push([
            text::float32(number, x)?,
            text::float32(number, y)?,
            text::float32(number, z)?,
        ]);
        Ok(())
    }

    fn face(&mut self, number: u64, rest: &str) -> Result<()> {
        self.count_record()?;
        let tokens: Vec<&str> = rest.split_ascii_whitespace().collect();
        if tokens.len() < 3 {
            return Err(at_line(codes::MESH_FACE_INVALID, number));
        }
        let length = len_u64(tokens.len());
        if length > self.limits.array_length {
            return Err(limit("array_length", length));
        }
        let mut corners = Vec::with_capacity(tokens.len());
        for token in &tokens {
            corners.push(self.corner(number, token)?);
        }
        if corners.len() > 3 {
            self.loss
                .add(reasons::POLYGON_TRIANGULATED, "face", Some(number), 1);
        }
        if let Some((first, others)) = corners.split_first() {
            for pair in others.windows(2) {
                if let [second, third] = pair {
                    self.triangle([*first, *second, *third])?;
                }
            }
        }
        Ok(())
    }

    fn triangle(&mut self, corners: [Corner; 3]) -> Result<()> {
        if len_u64(self.current.triangles.len()) >= self.limits.array_length {
            return Err(limit(
                "array_length",
                len_u64(self.current.triangles.len()) + 1,
            ));
        }
        let [a, b, c] = corners;
        let triangle = [
            self.current.vertex(a, &self.limits)?,
            self.current.vertex(b, &self.limits)?,
            self.current.vertex(c, &self.limits)?,
        ];
        self.current.triangles.push(triangle);
        Ok(())
    }

    fn corner(&self, number: u64, token: &str) -> Result<Corner> {
        let mut fields = token.split('/');
        let position = fields.next().unwrap_or_default();
        let uv = fields.next();
        let normal = fields.next();
        if fields.next().is_some() {
            return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
        }
        Ok(Corner {
            position: resolve(number, position, self.positions.len())?,
            uv: optional(number, uv, self.uvs.len())?,
            normal: optional(number, normal, self.normals.len())?,
        })
    }

    fn start_part(&mut self, name: String) {
        let next = Part::named(name, None);
        if self.current.has_faces() {
            let done = std::mem::replace(&mut self.current, next);
            self.finished.push(done);
        } else {
            self.current = next;
        }
    }

    fn use_material(&mut self, material: String) {
        if self.current.has_faces() {
            let next = Part::named(self.current.name.clone(), Some(material));
            let done = std::mem::replace(&mut self.current, next);
            self.finished.push(done);
        } else {
            self.current.material = Some(material);
        }
    }

    /// Builds the parts with faces, in file order, and returns them with the loss log.
    fn finish(mut self) -> Result<(Vec<MeshPart>, LossLog)> {
        if self.current.has_faces() {
            let last = std::mem::take(&mut self.current);
            self.finished.push(last);
        }
        let mut meshes = Vec::new();
        for part in std::mem::take(&mut self.finished) {
            meshes.push(self.build(part)?);
        }
        Ok((meshes, self.loss))
    }

    fn build(&mut self, part: Part) -> Result<MeshPart> {
        let mut points = Vec::with_capacity(part.vertices.len());
        for corner in &part.vertices {
            points.push(
                lookup(&self.positions, Some(corner.position))
                    .ok_or_else(|| Error::new(codes::MESH_INDEX_OUT_OF_RANGE))?,
            );
        }
        let count = part.vertices.len();
        let mut channels = Vec::new();
        let normals: Vec<Option<[f32; 3]>> = part
            .vertices
            .iter()
            .map(|c| lookup(&self.normals, c.normal))
            .collect();
        let missing = normals.iter().filter(|v| v.is_none()).count();
        if missing == 0 {
            let values: Vec<[f32; 3]> = normals.into_iter().flatten().collect();
            channels.push((AttributeKey::Normal, Channel::Vec3(values)));
        } else if missing < count {
            self.loss
                .attribute(reasons::ATTRIBUTE_INCOMPLETE, "normal", len_u64(missing));
        }
        let uvs: Vec<Option<[f32; 2]>> = part
            .vertices
            .iter()
            .map(|c| lookup(&self.uvs, c.uv))
            .collect();
        let missing = uvs.iter().filter(|v| v.is_none()).count();
        if missing == 0 {
            let values: Vec<[f32; 2]> = uvs.into_iter().flatten().collect();
            channels.push((AttributeKey::Uv(0), Channel::Vec2(values)));
        } else if missing < count {
            self.loss
                .attribute(reasons::ATTRIBUTE_INCOMPLETE, "uv", len_u64(missing));
        }
        let mesh = build_mesh(points, part.triangles, channels, &self.limits)?;
        Ok(MeshPart {
            name: part.name,
            mesh,
            material: part.material,
        })
    }
}

/// Splits a statement into its keyword and the rest, trimmed.
fn split_keyword(statement: &str) -> (&str, &str) {
    let statement = statement.trim_start();
    match statement.find(char::is_whitespace) {
        Some(at) => (
            statement.get(..at).unwrap_or_default(),
            statement.get(at..).unwrap_or_default().trim(),
        ),
        None => (statement, ""),
    }
}

fn check_numbers(number: u64, tokens: &[&str]) -> Result<()> {
    for token in tokens {
        text::float64(number, token)?;
    }
    Ok(())
}

fn lookup<T: Copy>(values: &[T], index: Option<u32>) -> Option<T> {
    values.get(usize::try_from(index?).ok()?).copied()
}

fn optional(number: u64, token: Option<&str>, count: usize) -> Result<Option<u32>> {
    match token {
        None | Some("") => Ok(None),
        Some(token) => resolve(number, token, count).map(Some),
    }
}

/// Resolves a one-based or negative (relative to the pool's end) index into a pool of `count`.
fn resolve(number: u64, token: &str, count: usize) -> Result<u32> {
    let value = text::integer(number, token)?;
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    let index = if value > 0 {
        value - 1
    } else if value < 0 {
        count + value
    } else {
        -1
    };
    let error = || at_line(codes::MESH_INDEX_OUT_OF_RANGE, number);
    if index < 0 || index >= count {
        return Err(error());
    }
    u32::try_from(index).map_err(|_| error())
}

/// Reads an MTL library.
fn read_mtl(
    data: &[u8],
    limits: &Limits,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<Vec<Material>> {
    let mut materials: Vec<Material> = Vec::new();
    let mut records = 0_u64;
    let total = len_u64(data.len());
    let mut lines = Lines::new(data, limits);
    while let Some(item) = lines.next() {
        let (number, raw) = item?;
        control.step(lines.offset(), total)?;
        let line = text::text(number, raw)?;
        let body = line.split('#').next().unwrap_or_default();
        let (keyword, rest) = split_keyword(body);
        if keyword.is_empty() {
            continue;
        }
        records = records.saturating_add(1);
        if records > limits.record_count {
            return Err(limit("record_count", records));
        }
        if keyword == "newmtl" {
            if rest.is_empty() {
                return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
            }
            materials.push(Material {
                name: rest.to_owned(),
                ..Material::default()
            });
            continue;
        }
        let material = materials
            .last_mut()
            .ok_or_else(|| at_line(codes::MESH_STRUCTURE_INVALID, number))?;
        apply_mtl_statement(material, number, keyword, rest, loss)?;
    }
    Ok(materials)
}

fn apply_mtl_statement(
    material: &mut Material,
    number: u64,
    keyword: &str,
    rest: &str,
    loss: &mut LossLog,
) -> Result<()> {
    match keyword {
        "Ka" | "Kd" | "Ks" => match color(number, rest)? {
            Some(value) => match keyword {
                "Ka" => material.ambient = Some(value),
                "Kd" => material.diffuse = Some(value),
                _ => material.specular = Some(value),
            },
            None => loss.add(reasons::ELEMENT_SKIPPED, keyword, Some(number), 1),
        },
        "d" => material.opacity = Some(last_number(number, rest)?),
        "Tr" => material.opacity = Some(1.0 - last_number(number, rest)?),
        "bump" | "disp" | "decal" | "refl" | "norm" => {
            push_texture(material, number, keyword, rest)?
        }
        slot if slot.starts_with("map_") => push_texture(material, number, keyword, rest)?,
        other => loss.add(reasons::ELEMENT_SKIPPED, other, Some(number), 1),
    }
    Ok(())
}

fn push_texture(material: &mut Material, number: u64, slot: &str, rest: &str) -> Result<()> {
    if rest.is_empty() {
        return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
    }
    material.textures.push(TextureMap {
        slot: slot.to_owned(),
        value: rest.to_owned(),
    });
    Ok(())
}

/// Three color components; other forms (spectral or CIE curves) give `None`.
fn color(number: u64, rest: &str) -> Result<Option<[f32; 3]>> {
    let fields: Vec<&str> = rest.split_ascii_whitespace().collect();
    match fields.as_slice() {
        [r, g, b] => Ok(Some([
            text::float32(number, r)?,
            text::float32(number, g)?,
            text::float32(number, b)?,
        ])),
        _ => Ok(None),
    }
}

/// The last number of a statement, so that an option such as `-halo` before it is skipped.
fn last_number(number: u64, rest: &str) -> Result<f32> {
    let token = rest.split_ascii_whitespace().last().unwrap_or_default();
    text::float32(number, token)
}

/// Writes a scene as OBJ, with the MTL library of its materials when it has any.
///
/// # Errors
///
/// `io.mesh.encoding_unsupported` for any encoding but ASCII, `io.mesh.write_value_invalid` for a
/// name the format cannot hold, and `base.cancelled`.
pub fn write(scene: &MeshScene, options: &mut WriteOptions<'_>) -> Result<WriteOutput> {
    if options.encoding != Encoding::Ascii {
        return Err(Error::new(codes::MESH_ENCODING_UNSUPPORTED));
    }
    let mut out = String::new();
    if let Some(name) = options
        .mtl_name
        .as_deref()
        .filter(|_| !scene.materials.is_empty())
    {
        obj_text(name, false)?;
        out.push_str(&format!("mtllib {name}\n"));
    }
    let mut control = options.control();
    let mut loss = LossLog::default();
    let total: u64 = scene
        .meshes
        .iter()
        .map(|part| len_u64(part.mesh.triangle_count()))
        .sum();
    let mut bases = Bases::default();
    let mut done = 0_u64;
    for part in &scene.meshes {
        if part.mesh.triangle_count() == 0 {
            loss.add(reasons::ELEMENT_SKIPPED, "part", None, 1);
            continue;
        }
        write_part(
            &mut out,
            part,
            &mut bases,
            &mut loss,
            (&mut done, total),
            &mut control,
        )?;
    }
    let mtl = if scene.materials.is_empty() {
        None
    } else {
        Some(write_mtl(&scene.materials)?)
    };
    control.finish(total);
    let report = loss.finish(LossHeader::new("obj", "", Direction::Export, None));
    Ok(WriteOutput {
        bytes: out.into_bytes(),
        mtl,
        report,
    })
}

/// The number of vertices written so far in each pool; OBJ indices count from one across the file.
#[derive(Default)]
struct Bases {
    position: u64,
    uv: u64,
    normal: u64,
}

fn write_part(
    out: &mut String,
    part: &MeshPart,
    bases: &mut Bases,
    loss: &mut LossLog,
    progress: (&mut u64, u64),
    control: &mut Control<'_>,
) -> Result<()> {
    obj_text(&part.name, true)?;
    if part.name.is_empty() {
        out.push_str("o\n");
    } else {
        out.push_str(&format!("o {}\n", part.name));
    }
    if let Some(material) = &part.material {
        obj_text(material, false)?;
        out.push_str(&format!("usemtl {material}\n"));
    }
    let mesh = &part.mesh;
    let vertices = len_u64(mesh.vertex_count());
    for key in mesh.channels().keys() {
        if !matches!(key, AttributeKey::Normal | AttributeKey::Uv(0)) {
            loss.attribute(reasons::ATTRIBUTE_DROPPED, attribute_name(key), vertices);
        }
    }
    for point in mesh.positions() {
        out.push_str(&format!(
            "v {} {} {}\n",
            text64(point.x),
            text64(point.y),
            text64(point.z)
        ));
    }
    let normals = mesh.normals();
    if let Some(values) = normals {
        for [x, y, z] in values {
            out.push_str(&format!(
                "vn {} {} {}\n",
                text32(*x),
                text32(*y),
                text32(*z)
            ));
        }
    }
    let uvs = mesh.uv_set(0);
    if let Some(values) = uvs {
        for [u, v] in values {
            out.push_str(&format!("vt {} {}\n", text32(*u), text32(*v)));
        }
    }
    let (uv, normal) = (uvs.is_some(), normals.is_some());
    let (done, total) = progress;
    for triangle in mesh.triangles() {
        let [a, b, c] = *triangle;
        out.push_str(&format!(
            "f {} {} {}\n",
            corner_text(bases, a, uv, normal),
            corner_text(bases, b, uv, normal),
            corner_text(bases, c, uv, normal)
        ));
        *done += 1;
        control.step(*done, total)?;
    }
    bases.position += vertices;
    if uv {
        bases.uv += vertices;
    }
    if normal {
        bases.normal += vertices;
    }
    Ok(())
}

/// The face corner text of vertex `index` of the part: `v`, `v/vt`, `v//vn` or `v/vt/vn`.
fn corner_text(bases: &Bases, index: u32, uv: bool, normal: bool) -> String {
    let local = u64::from(index) + 1;
    let position = bases.position + local;
    match (uv, normal) {
        (false, false) => format!("{position}"),
        (true, false) => format!("{position}/{}", bases.uv + local),
        (false, true) => format!("{position}//{}", bases.normal + local),
        (true, true) => format!("{position}/{}/{}", bases.uv + local, bases.normal + local),
    }
}

/// Fails with `io.mesh.write_value_invalid` unless an OBJ name reads back unchanged: no comment
/// character, no backslash (a continuation at a line end), and no line break.
fn obj_text(value: &str, allow_empty: bool) -> Result<()> {
    let bad = value.contains(['#', '\\']);
    if bad {
        return Err(Error::new(codes::MESH_WRITE_VALUE_INVALID));
    }
    mtl_text(value, allow_empty)
}

/// Fails with `io.mesh.write_value_invalid` unless an MTL name or value reads back unchanged: no
/// comment character and no line break or surrounding white space.
fn mtl_text(value: &str, allow_empty: bool) -> Result<()> {
    if (!allow_empty && value.is_empty()) || value.contains('#') {
        return Err(Error::new(codes::MESH_WRITE_VALUE_INVALID));
    }
    text::line_text(value)
}

fn write_mtl(materials: &[Material]) -> Result<Vec<u8>> {
    let mut out = String::new();
    for material in materials {
        mtl_text(&material.name, false)?;
        out.push_str(&format!("newmtl {}\n", material.name));
        for (keyword, value) in [
            ("Ka", material.ambient),
            ("Kd", material.diffuse),
            ("Ks", material.specular),
        ] {
            if let Some([r, g, b]) = value {
                out.push_str(&format!(
                    "{keyword} {} {} {}\n",
                    text32(r),
                    text32(g),
                    text32(b)
                ));
            }
        }
        if let Some(opacity) = material.opacity {
            out.push_str(&format!("d {}\n", text32(opacity)));
        }
        for texture in &material.textures {
            if texture.slot.is_empty() || texture.slot.contains(char::is_whitespace) {
                return Err(Error::new(codes::MESH_WRITE_VALUE_INVALID));
            }
            mtl_text(&texture.slot, false)?;
            mtl_text(&texture.value, false)?;
            out.push_str(&format!("{} {}\n", texture.slot, texture.value));
        }
    }
    Ok(out.into_bytes())
}
