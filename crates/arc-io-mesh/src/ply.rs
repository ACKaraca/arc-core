//! PLY polygon files (CORE-IO-025): ASCII, and binary little or big endian.
//!
//! Reading keeps the first `vertex` and first `face` elements. Positions come from `x y z`
//! (finite numbers, widened to `f64`), normals from `nx ny nz`, texture coordinates from `s t`,
//! `u v`, `texture_u texture_v` or `texture_s texture_t`, and colors from `red green blue` with an
//! optional `alpha`. Integer colors are divided by 255. Faces come from the list `vertex_indices`
//! (or `vertex_index`) and are fan-triangulated. Other elements and properties are skipped and
//! reported. The vertices keep their order, so face indices are unchanged.
//!
//! Writing emits one `vertex` element for all parts and one `face` element. Positions are `double`,
//! normals and texture coordinates `float`, colors `uchar` (rounded to 8 bits; the rounding is
//! reported). Part names and materials have no place in a PLY file and are reported as dropped.

use arc_base::math::Point3;
use arc_base::{Error, Limits, Result};
use arc_mesh::{AttributeKey, Channel};

use crate::binary::Bytes;
use crate::error::{at_line, at_offset, codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog};
use crate::model::{MeshPart, MeshScene, attribute_name, build_mesh};
use crate::options::{Control, Encoding, ReadOptions, ReadOutput, WriteOptions, WriteOutput};
use crate::text::{self, Lines, len_u64, narrow};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Ascii,
    LittleEndian,
    BigEndian,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Self::Ascii => "ascii",
            Self::LittleEndian => "binary_little_endian",
            Self::BigEndian => "binary_big_endian",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }

    fn size(self) -> u64 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

/// A value of a property: an integer or a finite floating-point number.
#[derive(Clone, Copy, Debug)]
enum Number {
    Int(i64),
    Float(f64),
}

impl Number {
    fn value(self) -> f64 {
        match self {
            Self::Int(v) => v as f64,
            Self::Float(v) => v,
        }
    }

    /// A color component: integers are bytes, floating-point values are already in [0, 1].
    fn color(self) -> f32 {
        match self {
            Self::Int(v) => byte_color(v),
            Self::Float(v) => narrow(v),
        }
    }
}

/// The color component of a byte. Writing and reading use this one mapping.
fn byte_color(value: i64) -> f32 {
    narrow(value as f64 / 255.0)
}

#[derive(Debug)]
enum Property {
    Scalar {
        name: String,
        ty: Scalar,
    },
    List {
        name: String,
        count: Scalar,
        item: Scalar,
    },
}

impl Property {
    fn name(&self) -> &str {
        match self {
            Self::Scalar { name, .. } | Self::List { name, .. } => name,
        }
    }

    fn record_bytes(&self) -> u64 {
        match self {
            Self::Scalar { ty, .. } => ty.size(),
            Self::List { count, .. } => count.size(),
        }
    }
}

#[derive(Debug)]
struct Element {
    name: String,
    count: u64,
    properties: Vec<Property>,
}

impl Element {
    /// The fewest bytes one record of this element can take in a binary file.
    fn record_bytes(&self) -> u64 {
        self.properties.iter().map(Property::record_bytes).sum()
    }
}

struct Header {
    format: Format,
    elements: Vec<Element>,
}

fn header_error(number: u64) -> arc_base::Error {
    at_line(codes::MESH_HEADER_INVALID, number)
}

/// Reads the header lines up to and including `end_header`.
fn parse_header(lines: &mut Lines<'_>, limits: &Limits) -> Result<Header> {
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut first = true;
    loop {
        let Some(item) = lines.next() else {
            return Err(header_error(lines.number()));
        };
        let (number, raw) = item?;
        let line = text::text(number, raw)?.trim();
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        if first {
            if fields != ["ply"] {
                return Err(header_error(number));
            }
            first = false;
            continue;
        }
        match fields.as_slice() {
            [] => {}
            ["format", name, "1.0"] => {
                format = Some(match *name {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::LittleEndian,
                    "binary_big_endian" => Format::BigEndian,
                    _ => return Err(header_error(number)),
                });
            }
            ["comment", ..] | ["obj_info", ..] => {}
            ["element", name, count] => {
                let count = count.parse::<u64>().map_err(|_| header_error(number))?;
                if len_u64(elements.len()) >= limits.array_length {
                    return Err(limit("array_length", len_u64(elements.len()) + 1));
                }
                elements.push(Element {
                    name: (*name).to_owned(),
                    count,
                    properties: Vec::new(),
                });
            }
            ["property", "list", count, item, name] => {
                let property = Property::List {
                    name: (*name).to_owned(),
                    count: Scalar::parse(count).ok_or_else(|| header_error(number))?,
                    item: Scalar::parse(item).ok_or_else(|| header_error(number))?,
                };
                push_property(&mut elements, property, number)?;
            }
            ["property", ty, name] => {
                let property = Property::Scalar {
                    name: (*name).to_owned(),
                    ty: Scalar::parse(ty).ok_or_else(|| header_error(number))?,
                };
                push_property(&mut elements, property, number)?;
            }
            ["end_header"] => break,
            _ => return Err(header_error(number)),
        }
    }
    let format = format.ok_or_else(|| header_error(lines.number()))?;
    Ok(Header { format, elements })
}

fn push_property(elements: &mut [Element], property: Property, number: u64) -> Result<()> {
    let element = elements.last_mut().ok_or_else(|| header_error(number))?;
    element.properties.push(property);
    Ok(())
}

/// Fails before any record is read when the header declares more records than the limits allow
/// or than the body can hold: each record of a binary element takes at least `record_bytes`
/// bytes, and each record of an ASCII element at least one byte.
fn check_counts(header: &Header, body_len: u64, limits: &Limits) -> Result<()> {
    let mut total = 0_u64;
    for element in &header.elements {
        total = total.saturating_add(element.count);
        if total > limits.record_count {
            return Err(limit("record_count", total));
        }
        let minimum = match header.format {
            Format::Ascii => element.count,
            _ => element.count.saturating_mul(element.record_bytes()),
        };
        if minimum > body_len {
            return Err(at_offset(codes::MESH_TRUNCATED, body_len));
        }
    }
    Ok(())
}

/// The body of an ASCII file: one non-blank line per record, read token by token.
struct AsciiBody<'a> {
    lines: Lines<'a>,
    fields: Vec<&'a str>,
    next: usize,
    line: u64,
}

/// The records of the body in either form.
enum Body<'a> {
    Ascii(AsciiBody<'a>),
    Binary(Bytes<'a>),
}

/// The body with the limits and the total used for progress.
struct Source<'a> {
    body: Body<'a>,
    limits: Limits,
    total: u64,
}

impl<'a> Source<'a> {
    fn offset(&self) -> u64 {
        match &self.body {
            Body::Ascii(ascii) => ascii.lines.offset(),
            Body::Binary(bytes) => bytes.offset(),
        }
    }

    /// Starts the next record: in ASCII, the next line that is not blank.
    fn begin(&mut self) -> Result<()> {
        let Body::Ascii(ascii) = &mut self.body else {
            return Ok(());
        };
        loop {
            let Some(item) = ascii.lines.next() else {
                return Err(at_offset(codes::MESH_TRUNCATED, self.total));
            };
            let (number, raw) = item?;
            let line = text::text(number, raw)?;
            if line.trim().is_empty() {
                continue;
            }
            ascii.fields.clear();
            ascii.fields.extend(line.split_ascii_whitespace());
            ascii.next = 0;
            ascii.line = number;
            return Ok(());
        }
    }

    /// The next value of a record.
    fn scalar(&mut self, ty: Scalar) -> Result<Number> {
        match &mut self.body {
            Body::Ascii(ascii) => {
                let token = *ascii
                    .fields
                    .get(ascii.next)
                    .ok_or_else(|| at_line(codes::MESH_STRUCTURE_INVALID, ascii.line))?;
                ascii.next += 1;
                match ty {
                    Scalar::F32 | Scalar::F64 => {
                        Ok(Number::Float(text::float64(ascii.line, token)?))
                    }
                    _ => Ok(Number::Int(text::integer(ascii.line, token)?)),
                }
            }
            Body::Binary(bytes) => {
                let number = match ty {
                    Scalar::I8 => Number::Int(i64::from(bytes.i8()?)),
                    Scalar::U8 => Number::Int(i64::from(bytes.u8()?)),
                    Scalar::I16 => Number::Int(i64::from(bytes.i16()?)),
                    Scalar::U16 => Number::Int(i64::from(bytes.u16()?)),
                    Scalar::I32 => Number::Int(i64::from(bytes.i32()?)),
                    Scalar::U32 => Number::Int(i64::from(bytes.u32()?)),
                    Scalar::F32 => Number::Float(f64::from(bytes.f32()?)),
                    Scalar::F64 => Number::Float(bytes.f64()?),
                };
                if let Number::Float(value) = number
                    && !value.is_finite()
                {
                    return Err(at_offset(codes::MESH_VALUE_INVALID, bytes.offset()));
                }
                Ok(number)
            }
        }
    }

    /// The length of a list, checked against the array limit and the bytes left.
    fn list_len(&mut self, count: Scalar, item: Scalar) -> Result<usize> {
        let length = match self.scalar(count)? {
            Number::Int(value) => u64::try_from(value).map_err(|_| self.invalid_value())?,
            Number::Float(_) => return Err(self.invalid_value()),
        };
        if length > self.limits.array_length {
            return Err(limit("array_length", length));
        }
        if let Body::Binary(bytes) = &self.body {
            let needed = length.saturating_mul(item.size());
            if needed > len_u64(bytes.remaining()) {
                return Err(at_offset(codes::MESH_TRUNCATED, bytes.offset()));
            }
        }
        usize::try_from(length).map_err(|_| limit("array_length", length))
    }

    fn invalid_value(&self) -> arc_base::Error {
        match &self.body {
            Body::Ascii(ascii) => at_line(codes::MESH_VALUE_INVALID, ascii.line),
            Body::Binary(bytes) => at_offset(codes::MESH_VALUE_INVALID, bytes.offset()),
        }
    }

    /// Ends the record: in ASCII, no token may be left over.
    fn end(&mut self) -> Result<()> {
        if let Body::Ascii(ascii) = &self.body
            && ascii.next != ascii.fields.len()
        {
            return Err(at_line(codes::MESH_STRUCTURE_INVALID, ascii.line));
        }
        Ok(())
    }

    /// Ends the body: nothing may follow the last record.
    fn finish(mut self) -> Result<()> {
        match &mut self.body {
            Body::Ascii(ascii) => {
                for item in ascii.lines.by_ref() {
                    let (number, raw) = item?;
                    if !text::text(number, raw)?.trim().is_empty() {
                        return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
                    }
                }
                Ok(())
            }
            Body::Binary(bytes) => {
                if bytes.remaining() > 0 {
                    Err(at_offset(codes::MESH_STRUCTURE_INVALID, bytes.offset()))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// What a vertex property is, when the model keeps it.
#[derive(Clone, Copy)]
enum Role {
    Position(usize),
    Normal(usize),
    Uv(usize),
    Color(usize),
}

fn role(name: &str) -> Option<Role> {
    Some(match name {
        "x" => Role::Position(0),
        "y" => Role::Position(1),
        "z" => Role::Position(2),
        "nx" => Role::Normal(0),
        "ny" => Role::Normal(1),
        "nz" => Role::Normal(2),
        "s" | "u" | "texture_u" | "texture_s" => Role::Uv(0),
        "t" | "v" | "texture_v" | "texture_t" => Role::Uv(1),
        "red" | "r" => Role::Color(0),
        "green" | "g" => Role::Color(1),
        "blue" | "b" => Role::Color(2),
        "alpha" | "a" => Role::Color(3),
        _ => return None,
    })
}

/// The vertices of the first `vertex` element, with the attributes the file carries.
#[derive(Default)]
struct Vertices {
    positions: Vec<Point3>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    has_normal: bool,
    has_uv: bool,
    has_color: bool,
}

fn read_vertices(
    source: &mut Source<'_>,
    element: &Element,
    out: &mut Vertices,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<()> {
    let roles: Vec<Option<Role>> = element
        .properties
        .iter()
        .map(|property| match property {
            Property::Scalar { name, .. } => role(name),
            Property::List { .. } => None,
        })
        .collect();
    let has = |wanted: &[Role]| {
        wanted.iter().all(|w| {
            roles.iter().any(|r| match (r, w) {
                (Some(Role::Position(a)), Role::Position(b))
                | (Some(Role::Normal(a)), Role::Normal(b))
                | (Some(Role::Uv(a)), Role::Uv(b))
                | (Some(Role::Color(a)), Role::Color(b)) => a == b,
                _ => false,
            })
        })
    };
    let position = [Role::Position(0), Role::Position(1), Role::Position(2)];
    if element.count > 0 && !has(&position) {
        return Err(Error::new(codes::MESH_STRUCTURE_INVALID));
    }
    out.has_normal = has(&[Role::Normal(0), Role::Normal(1), Role::Normal(2)]);
    out.has_uv = has(&[Role::Uv(0), Role::Uv(1)]);
    out.has_color = has(&[Role::Color(0), Role::Color(1), Role::Color(2)]);
    for _ in 0..element.count {
        source.begin()?;
        let mut point = [0.0_f64; 3];
        let mut normal = [0.0_f32; 3];
        let mut uv = [0.0_f32; 2];
        let mut color = [0.0_f32, 0.0, 0.0, 1.0];
        for (property, role) in element.properties.iter().zip(&roles) {
            match property {
                Property::Scalar { ty, .. } => {
                    let number = source.scalar(*ty)?;
                    match role {
                        Some(Role::Position(i)) => store(&mut point, *i, number.value()),
                        Some(Role::Normal(i)) => store(&mut normal, *i, narrow(number.value())),
                        Some(Role::Uv(i)) => store(&mut uv, *i, narrow(number.value())),
                        Some(Role::Color(i)) => store(&mut color, *i, number.color()),
                        None => {}
                    }
                }
                Property::List { count, item, .. } => {
                    let length = source.list_len(*count, *item)?;
                    for _ in 0..length {
                        source.scalar(*item)?;
                    }
                }
            }
        }
        source.end()?;
        let [x, y, z] = point;
        out.positions.push(Point3::new(x, y, z));
        if out.has_normal {
            out.normals.push(normal);
        }
        if out.has_uv {
            out.uvs.push(uv);
        }
        if out.has_color {
            out.colors.push(color);
        }
        control.step(source.offset(), source.total)?;
    }
    skipped_properties(element, roles.as_slice(), loss);
    Ok(())
}

/// Stores a value in a fixed-size array slot; an index outside the array is ignored.
fn store<T>(slots: &mut [T], index: usize, value: T) {
    if let Some(slot) = slots.get_mut(index) {
        *slot = value;
    }
}

/// Adds one `property_skipped` item per property of an element the model does not hold.
fn skipped_properties(element: &Element, roles: &[Option<Role>], loss: &mut LossLog) {
    for (property, role) in element.properties.iter().zip(roles) {
        if role.is_none() {
            loss.add(
                reasons::PROPERTY_SKIPPED,
                property.name(),
                None,
                element.count,
            );
        }
    }
}

/// The face indices of the first `face` element, fan-triangulated; indices are checked later.
fn read_faces(
    source: &mut Source<'_>,
    element: &Element,
    faces: &mut Vec<[u64; 3]>,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<()> {
    let list = element.properties.iter().position(|property| {
        matches!(property, Property::List { name, .. } if name == "vertex_indices" || name == "vertex_index")
    });
    if element.count > 0 && list.is_none() {
        return Err(Error::new(codes::MESH_STRUCTURE_INVALID));
    }
    let mut corners: Vec<u64> = Vec::new();
    for _ in 0..element.count {
        source.begin()?;
        corners.clear();
        for (index, property) in element.properties.iter().enumerate() {
            match property {
                Property::Scalar { ty, .. } => {
                    source.scalar(*ty)?;
                }
                Property::List { count, item, .. } => {
                    let length = source.list_len(*count, *item)?;
                    for _ in 0..length {
                        let number = source.scalar(*item)?;
                        if Some(index) == list {
                            corners.push(vertex_index(number, source)?);
                        }
                    }
                }
            }
        }
        source.end()?;
        push_fan(&corners, faces, &source.limits, loss)?;
        control.step(source.offset(), source.total)?;
    }
    for (index, property) in element.properties.iter().enumerate() {
        if Some(index) != list {
            loss.add(
                reasons::PROPERTY_SKIPPED,
                property.name(),
                None,
                element.count,
            );
        }
    }
    Ok(())
}

fn vertex_index(number: Number, source: &Source<'_>) -> Result<u64> {
    match number {
        Number::Int(value) => u64::try_from(value).map_err(|_| source.invalid_value()),
        Number::Float(_) => Err(source.invalid_value()),
    }
}

fn push_fan(
    corners: &[u64],
    faces: &mut Vec<[u64; 3]>,
    limits: &Limits,
    loss: &mut LossLog,
) -> Result<()> {
    let count = len_u64(corners.len());
    if count < 3 {
        return Err(Error::new(codes::MESH_FACE_INVALID));
    }
    if count > limits.array_length {
        return Err(limit("array_length", count));
    }
    if count > 3 {
        loss.add(reasons::POLYGON_TRIANGULATED, "face", None, 1);
    }
    if let Some((first, others)) = corners.split_first() {
        for pair in others.windows(2) {
            if let [second, third] = pair {
                faces.push([*first, *second, *third]);
            }
        }
    }
    Ok(())
}

/// Builds the one part of a PLY file: vertices with their attributes and the checked faces.
fn build(
    vertices: Vertices,
    faces: &[[u64; 3]],
    limits: &Limits,
    loss: &mut LossLog,
) -> Result<Vec<MeshPart>> {
    let count = len_u64(vertices.positions.len());
    if faces.is_empty() {
        loss.add(reasons::ELEMENT_SKIPPED, "vertex", None, count);
        return Ok(Vec::new());
    }
    let mut triangles = Vec::with_capacity(faces.len());
    for face in faces {
        let mut corners = [0_u32; 3];
        for (slot, value) in corners.iter_mut().zip(face) {
            if *value >= count {
                return Err(Error::new(codes::MESH_INDEX_OUT_OF_RANGE));
            }
            *slot =
                u32::try_from(*value).map_err(|_| Error::new(codes::MESH_INDEX_OUT_OF_RANGE))?;
        }
        triangles.push(corners);
    }
    let mut channels = Vec::new();
    if vertices.has_normal {
        channels.push((AttributeKey::Normal, Channel::Vec3(vertices.normals)));
    }
    if vertices.has_uv {
        channels.push((AttributeKey::Uv(0), Channel::Vec2(vertices.uvs)));
    }
    if vertices.has_color {
        channels.push((AttributeKey::Color, Channel::Vec4(vertices.colors)));
    }
    let mesh = build_mesh(vertices.positions, triangles, channels, limits)?;
    Ok(vec![MeshPart {
        name: String::new(),
        mesh,
        material: None,
    }])
}

/// Reads a PLY file, ASCII or binary, from bytes.
///
/// # Errors
///
/// `io.mesh.header_invalid` for a malformed header, `io.limit_exceeded` naming the limit,
/// `io.mesh.truncated` when the body ends inside a record or a declared count cannot fit,
/// `io.mesh.index_out_of_range`, `io.mesh.face_invalid`, `io.mesh.value_invalid`,
/// `io.mesh.structure_invalid`, `io.mesh.text_invalid` and `base.cancelled`.
pub fn read(data: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits = options.limits;
    text::check_input(data.len(), &limits)?;
    let mut control = options.control();
    let mut loss = LossLog::default();
    let mut lines = Lines::new(data, &limits);
    let header = parse_header(&mut lines, &limits)?;
    let body_start = usize::try_from(lines.offset()).unwrap_or(data.len());
    let body = data.get(body_start..).unwrap_or_default();
    check_counts(&header, len_u64(body.len()), &limits)?;
    let total = len_u64(data.len());
    let body = match header.format {
        Format::Ascii => Body::Ascii(AsciiBody {
            lines,
            fields: Vec::new(),
            next: 0,
            line: 0,
        }),
        Format::LittleEndian => Body::Binary(Bytes::new(body, false)),
        Format::BigEndian => Body::Binary(Bytes::new(body, true)),
    };
    let mut source = Source {
        body,
        limits,
        total,
    };
    let mut vertices = Vertices::default();
    let mut faces: Vec<[u64; 3]> = Vec::new();
    let (mut seen_vertex, mut seen_face) = (false, false);
    for element in &header.elements {
        match element.name.as_str() {
            "vertex" if !seen_vertex => {
                seen_vertex = true;
                read_vertices(&mut source, element, &mut vertices, &mut control, &mut loss)?;
            }
            "face" if !seen_face => {
                seen_face = true;
                read_faces(&mut source, element, &mut faces, &mut control, &mut loss)?;
            }
            name => {
                skip_element(&mut source, element, &mut control)?;
                loss.add(reasons::ELEMENT_SKIPPED, name, None, element.count);
            }
        }
    }
    source.finish()?;
    control.finish(total);
    let meshes = build(vertices, &faces, &limits, &mut loss)?;
    let report = loss.finish(LossHeader::new(
        "ply",
        "1.0",
        Direction::Import,
        Some(limits),
    ));
    Ok(ReadOutput {
        scene: MeshScene {
            meshes,
            materials: Vec::new(),
        },
        report,
    })
}

fn skip_element(
    source: &mut Source<'_>,
    element: &Element,
    control: &mut Control<'_>,
) -> Result<()> {
    for _ in 0..element.count {
        source.begin()?;
        for property in &element.properties {
            match property {
                Property::Scalar { ty, .. } => {
                    source.scalar(*ty)?;
                }
                Property::List { count, item, .. } => {
                    let length = source.list_len(*count, *item)?;
                    for _ in 0..length {
                        source.scalar(*item)?;
                    }
                }
            }
        }
        source.end()?;
        control.step(source.offset(), source.total)?;
    }
    Ok(())
}

/// Writes a scene as PLY in the requested encoding.
///
/// # Errors
///
/// `io.mesh.write_value_invalid` when the vertices exceed the 32-bit index range, and
/// `base.cancelled`.
pub fn write(scene: &MeshScene, options: &mut WriteOptions<'_>) -> Result<WriteOutput> {
    let format = match options.encoding {
        Encoding::Ascii => Format::Ascii,
        Encoding::BinaryLittleEndian => Format::LittleEndian,
        Encoding::BinaryBigEndian => Format::BigEndian,
    };
    let parts: Vec<&MeshPart> = scene
        .meshes
        .iter()
        .filter(|part| part.mesh.triangle_count() > 0)
        .collect();
    let mut loss = LossLog::default();
    loss.add(
        reasons::ELEMENT_SKIPPED,
        "part",
        None,
        len_u64(scene.meshes.len() - parts.len()),
    );
    let normal = parts
        .iter()
        .all(|p| p.mesh.channel(&AttributeKey::Normal).is_some());
    let uv = parts
        .iter()
        .all(|p| p.mesh.channel(&AttributeKey::Uv(0)).is_some());
    let color = parts
        .iter()
        .all(|p| p.mesh.channel(&AttributeKey::Color).is_some());
    let vertices: u64 = parts.iter().map(|p| len_u64(p.mesh.vertex_count())).sum();
    let faces: u64 = parts.iter().map(|p| len_u64(p.mesh.triangle_count())).sum();
    if u32::try_from(vertices).is_err() {
        return Err(Error::new(codes::MESH_WRITE_VALUE_INVALID));
    }
    let mut named = 0_u64;
    for part in &parts {
        let count = len_u64(part.mesh.vertex_count());
        for key in part.mesh.channels().keys() {
            let kept = match key {
                AttributeKey::Normal => normal,
                AttributeKey::Uv(0) => uv,
                AttributeKey::Color => color,
                _ => false,
            };
            if !kept {
                loss.attribute(reasons::ATTRIBUTE_DROPPED, attribute_name(key), count);
            }
        }
        if !part.name.is_empty() {
            named += 1;
        }
    }
    loss.attribute(reasons::ATTRIBUTE_DROPPED, "name", named);
    let materials = scene.materials.len() + parts.iter().filter(|p| p.material.is_some()).count();
    if materials > 0 {
        loss.attribute(reasons::ATTRIBUTE_DROPPED, "material", len_u64(materials));
    }
    let mut control = options.control();
    let mut out = header(format, vertices, faces, normal, uv, color).into_bytes();
    let mut rounded = 0_u64;
    let mut offset = 0_u64;
    let mut done = 0_u64;
    for part in &parts {
        for (index, point) in part.mesh.positions().iter().enumerate() {
            let normal_value = part.mesh.normals().and_then(|v| v.get(index)).copied();
            let uv_value = part.mesh.uv_set(0).and_then(|v| v.get(index)).copied();
            let color_value = part
                .mesh
                .channel(&AttributeKey::Color)
                .and_then(|c| match c {
                    Channel::Vec4(v) => v.get(index).copied(),
                    _ => None,
                });
            let bytes = Vertex {
                point: [point.x, point.y, point.z],
                normal: normal.then_some(normal_value).flatten(),
                uv: uv.then_some(uv_value).flatten(),
                color: color.then_some(color_value).flatten(),
            };
            write_vertex(&mut out, &bytes, format, &mut rounded);
        }
    }
    for part in &parts {
        for triangle in part.mesh.triangles() {
            write_face(&mut out, *triangle, offset, format);
            done += 1;
            control.step(done, faces)?;
        }
        offset += len_u64(part.mesh.vertex_count());
    }
    loss.attribute(reasons::PRECISION_REDUCED, "color", rounded);
    control.finish(faces);
    let report = loss.finish(LossHeader::new("ply", "1.0", Direction::Export, None));
    Ok(WriteOutput {
        bytes: out,
        mtl: None,
        report,
    })
}

/// One vertex to write: its position and the attributes the file keeps.
struct Vertex {
    point: [f64; 3],
    normal: Option<[f32; 3]>,
    uv: Option<[f32; 2]>,
    color: Option<[f32; 4]>,
}

fn header(
    format: Format,
    vertices: u64,
    faces: u64,
    normal: bool,
    uv: bool,
    color: bool,
) -> String {
    let mut text = format!(
        "ply\nformat {} 1.0\ncomment written by arc-io-mesh\nelement vertex {vertices}\n",
        format.name()
    );
    text.push_str("property double x\nproperty double y\nproperty double z\n");
    if normal {
        text.push_str("property float nx\nproperty float ny\nproperty float nz\n");
    }
    if uv {
        text.push_str("property float s\nproperty float t\n");
    }
    if color {
        text.push_str(
            "property uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\n",
        );
    }
    text.push_str(&format!(
        "element face {faces}\nproperty list uchar uint vertex_indices\nend_header\n"
    ));
    text
}

/// Rounds a color component to a byte, the nearest value the reader maps back to the input.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the byte range first"
)]
fn to_byte(value: f32) -> u8 {
    (f64::from(value) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn write_vertex(out: &mut Vec<u8>, vertex: &Vertex, format: Format, rounded: &mut u64) {
    let mut colors = [0_u8; 4];
    if let Some(color) = vertex.color {
        for (slot, value) in colors.iter_mut().zip(color) {
            *slot = to_byte(value);
        }
        if colors
            .iter()
            .zip(color)
            .any(|(byte, value)| byte_color(i64::from(*byte)) != value)
        {
            *rounded += 1;
        }
    }
    match format {
        Format::Ascii => {
            let mut line = vertex.point.map(crate::text::text64).join(" ");
            if let Some(normal) = vertex.normal {
                line.push_str(&format!(" {}", normal.map(crate::text::text32).join(" ")));
            }
            if let Some(uv) = vertex.uv {
                line.push_str(&format!(" {}", uv.map(crate::text::text32).join(" ")));
            }
            if vertex.color.is_some() {
                line.push_str(&format!(" {}", colors.map(|c| c.to_string()).join(" ")));
            }
            line.push('\n');
            out.extend_from_slice(line.as_bytes());
        }
        Format::LittleEndian | Format::BigEndian => {
            let big = format == Format::BigEndian;
            for value in vertex.point {
                push_f64(out, value, big);
            }
            if let Some(normal) = vertex.normal {
                for value in normal {
                    push_f32(out, value, big);
                }
            }
            if let Some(uv) = vertex.uv {
                for value in uv {
                    push_f32(out, value, big);
                }
            }
            if vertex.color.is_some() {
                out.extend_from_slice(&colors);
            }
        }
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32, big: bool) {
    out.extend_from_slice(&if big {
        value.to_be_bytes()
    } else {
        value.to_le_bytes()
    });
}

fn push_f64(out: &mut Vec<u8>, value: f64, big: bool) {
    out.extend_from_slice(&if big {
        value.to_be_bytes()
    } else {
        value.to_le_bytes()
    });
}

fn write_face(out: &mut Vec<u8>, triangle: [u32; 3], offset: u64, format: Format) {
    let indices = triangle.map(|i| u64::from(i) + offset);
    match format {
        Format::Ascii => {
            let [a, b, c] = indices;
            let line = format!("3 {a} {b} {c}\n");
            out.extend_from_slice(line.as_bytes());
        }
        Format::LittleEndian | Format::BigEndian => {
            out.push(3);
            let big = format == Format::BigEndian;
            for index in indices {
                let value = u32::try_from(index).unwrap_or(u32::MAX);
                out.extend_from_slice(&if big {
                    value.to_be_bytes()
                } else {
                    value.to_le_bytes()
                });
            }
        }
    }
}
