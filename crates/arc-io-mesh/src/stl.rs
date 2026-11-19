//! STL triangle files (CORE-IO-025), ASCII and binary.
//!
//! A file is binary when its length is 84 bytes plus 50 bytes for each facet, the facet count
//! being the little-endian `u32` at bytes 80 to 84; any other file is read as ASCII. The format
//! stores single-precision coordinates, so reading produces `f32` values widened to `f64`, and
//! writing rounds positions to single precision (reported as `io.mesh.precision_reduced`). Corners
//! with equal coordinates share one vertex. Facet normals and attribute bytes are not kept; a
//! non-zero one is reported as dropped, and writers compute the normal of each facet.
//!
//! Each ASCII solid is one part named by the solid's name. A binary file holds one solid, so
//! writing it drops the part names and the division into parts, and reports both.

use std::collections::HashMap;

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result};

use crate::binary::Bytes;
use crate::error::{at_line, codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog};
use crate::model::{MeshPart, MeshScene, attribute_name, build_mesh};
use crate::options::{Control, Encoding, ReadOptions, ReadOutput, WriteOptions, WriteOutput};
use crate::text::{self, Lines, len_u64, narrow};

const HEADER_BYTES: usize = 84;
const FACET_BYTES: u64 = 50;

/// The facet count of a binary file, when the length of the file matches it.
fn binary_facets(data: &[u8]) -> Option<u64> {
    let count = <[u8; 4]>::try_from(data.get(80..84)?).ok()?;
    let facets = u64::from(u32::from_le_bytes(count));
    let expected = len_u64(HEADER_BYTES) + FACET_BYTES * facets;
    (expected == len_u64(data.len())).then_some(facets)
}

/// The vertices and triangles of one solid, with equal coordinates welded.
#[derive(Default)]
struct Solid {
    name: String,
    positions: Vec<Point3>,
    index: HashMap<[u32; 3], u32>,
    triangles: Vec<[u32; 3]>,
}

impl Solid {
    fn vertex(&mut self, coordinates: [f32; 3], limits: &Limits) -> Result<u32> {
        // The key ignores the sign of zero, as the mesh does.
        let key = coordinates.map(|c| if c == 0.0 { 0 } else { c.to_bits() });
        if let Some(&vertex) = self.index.get(&key) {
            return Ok(vertex);
        }
        if len_u64(self.positions.len()) >= limits.array_length {
            return Err(limit("array_length", len_u64(self.positions.len()) + 1));
        }
        let next = u32::try_from(self.positions.len())
            .map_err(|_| limit("array_length", len_u64(self.positions.len()) + 1))?;
        let [x, y, z] = coordinates;
        self.positions
            .push(Point3::new(f64::from(x), f64::from(y), f64::from(z)));
        self.index.insert(key, next);
        Ok(next)
    }

    fn push_triangle(&mut self, corners: [[f32; 3]; 3], limits: &Limits) -> Result<()> {
        if len_u64(self.triangles.len()) >= limits.array_length {
            return Err(limit("array_length", len_u64(self.triangles.len()) + 1));
        }
        let [a, b, c] = corners;
        let triangle = [
            self.vertex(a, limits)?,
            self.vertex(b, limits)?,
            self.vertex(c, limits)?,
        ];
        self.triangles.push(triangle);
        Ok(())
    }

    /// The solid as a part, or nothing when it has no facets.
    fn into_part(self, limits: &Limits) -> Result<Option<MeshPart>> {
        if self.triangles.is_empty() {
            return Ok(None);
        }
        let mesh = build_mesh(self.positions, self.triangles, Vec::new(), limits)?;
        Ok(Some(MeshPart {
            name: self.name,
            mesh,
            material: None,
        }))
    }
}

/// Reads an STL file, binary or ASCII, from bytes.
///
/// # Errors
///
/// `io.limit_exceeded` naming the limit, `io.mesh.truncated` and the other `io.mesh.*` codes for
/// malformed input, and `base.cancelled`.
pub fn read(data: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits = options.limits;
    text::check_input(data.len(), &limits)?;
    let mut control = options.control();
    let mut loss = LossLog::default();
    let parts = match binary_facets(data) {
        Some(facets) => read_binary(data, facets, &limits, &mut control, &mut loss)?,
        None => read_ascii(data, &limits, &mut control, &mut loss)?,
    };
    control.finish(len_u64(data.len()));
    let report = loss.finish(LossHeader::new("stl", "", Direction::Import, Some(limits)));
    Ok(ReadOutput {
        scene: MeshScene {
            meshes: parts,
            materials: Vec::new(),
        },
        report,
    })
}

fn read_binary(
    data: &[u8],
    facets: u64,
    limits: &Limits,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<Vec<MeshPart>> {
    if facets > limits.record_count {
        return Err(limit("record_count", facets));
    }
    if facets > limits.array_length {
        return Err(limit("array_length", facets));
    }
    let mut bytes = Bytes::new(data.get(HEADER_BYTES..).unwrap_or_default(), false);
    let mut solid = Solid::default();
    let (mut normals, mut attributes) = (0_u64, 0_u64);
    let total = len_u64(data.len());
    for _ in 0..facets {
        control.step(bytes.offset(), total)?;
        let normal = read_vector(&mut bytes)?;
        let corners = [
            read_vector(&mut bytes)?,
            read_vector(&mut bytes)?,
            read_vector(&mut bytes)?,
        ];
        if bytes.u16()? != 0 {
            attributes += 1;
        }
        if normal != [0.0; 3] {
            normals += 1;
        }
        solid.push_triangle(corners, limits)?;
    }
    loss.attribute(reasons::ATTRIBUTE_DROPPED, "facet normal", normals);
    loss.attribute(reasons::ATTRIBUTE_DROPPED, "facet attribute", attributes);
    Ok(solid.into_part(limits)?.into_iter().collect())
}

fn read_vector(bytes: &mut Bytes<'_>) -> Result<[f32; 3]> {
    let mut vector = [0.0_f32; 3];
    for component in &mut vector {
        let value = bytes.f32()?;
        if !value.is_finite() {
            return Err(crate::error::at_offset(
                codes::MESH_VALUE_INVALID,
                bytes.offset(),
            ));
        }
        *component = value;
    }
    Ok(vector)
}

fn read_ascii(
    data: &[u8],
    limits: &Limits,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<Vec<MeshPart>> {
    let mut parts = Vec::new();
    let mut solid: Option<Solid> = None;
    let mut facet: Option<Vec<[f32; 3]>> = None;
    let mut facets = 0_u64;
    let mut normals = 0_u64;
    let total = len_u64(data.len());
    let mut lines = Lines::new(data, limits);
    while let Some(item) = lines.next() {
        let (number, raw) = item?;
        control.step(lines.offset(), total)?;
        let line = text::text(number, raw)?.trim();
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        match fields.as_slice() {
            [] => {}
            ["solid", ..] if solid.is_none() => {
                solid = Some(Solid {
                    name: line.get(5..).unwrap_or_default().trim().to_owned(),
                    ..Solid::default()
                });
            }
            ["endsolid", ..] if facet.is_none() => {
                let finished = solid
                    .take()
                    .ok_or_else(|| at_line(codes::MESH_STRUCTURE_INVALID, number))?;
                parts.extend(finished.into_part(limits)?);
            }
            ["facet", "normal", x, y, z] if solid.is_some() && facet.is_none() => {
                let normal = [
                    text::float32(number, x)?,
                    text::float32(number, y)?,
                    text::float32(number, z)?,
                ];
                if normal != [0.0; 3] {
                    normals += 1;
                }
                facets += 1;
                if facets > limits.record_count {
                    return Err(limit("record_count", facets));
                }
                facet = Some(Vec::with_capacity(3));
            }
            ["outer", "loop"] | ["endloop"] if facet.is_some() => {}
            ["vertex", x, y, z] if facet.as_ref().is_some_and(|v| v.len() < 3) => {
                let point = [
                    text::float32(number, x)?,
                    text::float32(number, y)?,
                    text::float32(number, z)?,
                ];
                facet.get_or_insert_with(Vec::new).push(point);
            }
            ["endfacet"] => {
                let corners = facet.take().unwrap_or_default();
                let (Some(solid), [a, b, c]) = (solid.as_mut(), corners.as_slice()) else {
                    return Err(at_line(codes::MESH_STRUCTURE_INVALID, number));
                };
                solid.push_triangle([*a, *b, *c], limits)?;
            }
            _ => return Err(at_line(codes::MESH_STRUCTURE_INVALID, number)),
        }
    }
    if solid.is_some() || facet.is_some() {
        return Err(crate::error::at_offset(codes::MESH_TRUNCATED, total));
    }
    loss.attribute(reasons::ATTRIBUTE_DROPPED, "facet normal", normals);
    Ok(parts)
}

/// Writes an STL file. ASCII writes one solid per part; binary writes one solid for all parts.
///
/// # Errors
///
/// `io.mesh.encoding_unsupported` for big-endian binary, `io.mesh.write_value_invalid` for a
/// position that single precision cannot hold or a name with a line break, and `base.cancelled`.
pub fn write(scene: &MeshScene, options: &mut WriteOptions<'_>) -> Result<WriteOutput> {
    if options.encoding == Encoding::BinaryBigEndian {
        return Err(Error::new(codes::MESH_ENCODING_UNSUPPORTED));
    }
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
    let binary = options.encoding == Encoding::BinaryLittleEndian;
    let total: u64 = parts.iter().map(|p| len_u64(p.mesh.triangle_count())).sum();
    let mut control = options.control();
    let mut named = 0_u64;
    for part in &parts {
        for key in part.mesh.channels().keys() {
            loss.attribute(
                reasons::ATTRIBUTE_DROPPED,
                attribute_name(key),
                len_u64(part.mesh.vertex_count()),
            );
        }
        if !part.name.is_empty() {
            named += 1;
        }
    }
    if !scene.materials.is_empty() || parts.iter().any(|p| p.material.is_some()) {
        let count = len_u64(scene.materials.len())
            + len_u64(parts.iter().filter(|p| p.material.is_some()).count());
        loss.attribute(reasons::ATTRIBUTE_DROPPED, "material", count);
    }
    let bytes = if binary {
        if parts.len() > 1 {
            loss.attribute(
                reasons::ATTRIBUTE_DROPPED,
                "part boundary",
                len_u64(parts.len()),
            );
        }
        loss.attribute(reasons::ATTRIBUTE_DROPPED, "name", named);
        write_binary(&parts, total, &mut control, &mut loss)?
    } else {
        write_ascii(&parts, total, &mut control, &mut loss)?
    };
    control.finish(total);
    let report = loss.finish(LossHeader::new("stl", "", Direction::Export, None));
    Ok(WriteOutput {
        bytes,
        mtl: None,
        report,
    })
}

/// The vertices of a part in single precision, reporting the values that were rounded.
fn single_positions(part: &MeshPart, loss: &mut LossLog) -> Result<Vec<[f32; 3]>> {
    let mut rounded = 0_u64;
    let mut points = Vec::with_capacity(part.mesh.vertex_count());
    for point in part.mesh.positions() {
        let coordinates = [point.x, point.y, point.z];
        let single = coordinates.map(narrow);
        if single.iter().any(|c| !c.is_finite()) {
            return Err(Error::new(codes::MESH_WRITE_VALUE_INVALID));
        }
        if single
            .iter()
            .zip(coordinates)
            .any(|(s, c)| f64::from(*s) != c)
        {
            rounded += 1;
        }
        points.push(single);
    }
    loss.attribute(reasons::PRECISION_REDUCED, "position", rounded);
    Ok(points)
}

fn corner(points: &[[f32; 3]], index: u32) -> Result<[f32; 3]> {
    points
        .get(usize::try_from(index).unwrap_or(usize::MAX))
        .copied()
        .ok_or_else(|| Error::new(codes::MESH_WRITE_VALUE_INVALID))
}

/// The unit normal of a facet in single precision, or zero where it is not representable.
fn facet_normal(corners: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = corners.map(|[x, y, z]| Point3::new(f64::from(x), f64::from(y), f64::from(z)));
    let normal: Vector3 = (b - a).cross(&(c - a));
    let scale = normal.amax();
    if !(scale > 0.0 && scale.is_finite()) {
        return [0.0; 3];
    }
    let scaled = normal / scale;
    let unit = scaled / scaled.norm();
    [narrow(unit.x), narrow(unit.y), narrow(unit.z)]
}

fn write_ascii(
    parts: &[&MeshPart],
    total: u64,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<Vec<u8>> {
    let mut out = String::new();
    let mut done = 0_u64;
    for part in parts {
        let name = solid_name(&part.name)?;
        let points = single_positions(part, loss)?;
        out.push_str(&format!("solid{name}\n"));
        for triangle in part.mesh.triangles() {
            let [i, j, k] = *triangle;
            let corners = [
                corner(&points, i)?,
                corner(&points, j)?,
                corner(&points, k)?,
            ];
            let [nx, ny, nz] = facet_normal(corners);
            out.push_str(&format!(
                "  facet normal {} {} {}\n    outer loop\n",
                text::text32(nx),
                text::text32(ny),
                text::text32(nz)
            ));
            for [x, y, z] in corners {
                out.push_str(&format!(
                    "      vertex {} {} {}\n",
                    text::text32(x),
                    text::text32(y),
                    text::text32(z)
                ));
            }
            out.push_str("    endloop\n  endfacet\n");
            done += 1;
            control.step(done, total)?;
        }
        out.push_str(&format!("endsolid{name}\n"));
    }
    Ok(out.into_bytes())
}

/// The name written after `solid` or `endsolid`, with its leading space, or nothing.
fn solid_name(name: &str) -> Result<String> {
    text::line_text(name)?;
    Ok(if name.is_empty() {
        String::new()
    } else {
        format!(" {name}")
    })
}

fn write_binary(
    parts: &[&MeshPart],
    total: u64,
    control: &mut Control<'_>,
    loss: &mut LossLog,
) -> Result<Vec<u8>> {
    let count = u32::try_from(total).map_err(|_| Error::new(codes::MESH_WRITE_VALUE_INVALID))?;
    let mut out = Vec::with_capacity(HEADER_BYTES + 50 * usize::try_from(total).unwrap_or(0));
    let mut header = [0_u8; 80];
    let label = b"arc-io-mesh binary STL";
    for (slot, byte) in header.iter_mut().zip(label) {
        *slot = *byte;
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&count.to_le_bytes());
    let mut done = 0_u64;
    for part in parts {
        let points = single_positions(part, loss)?;
        for triangle in part.mesh.triangles() {
            let [i, j, k] = *triangle;
            let corners = [
                corner(&points, i)?,
                corner(&points, j)?,
                corner(&points, k)?,
            ];
            for value in facet_normal(corners)
                .into_iter()
                .chain(corners.into_iter().flatten())
            {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&0_u16.to_le_bytes());
            done += 1;
            control.step(done, total)?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::binary_facets;

    #[test]
    fn length_decides_binary_not_the_header_word() {
        let mut file = vec![0_u8; 84];
        file[..5].copy_from_slice(b"solid");
        assert_eq!(binary_facets(&file), Some(0));
        file.push(0);
        assert_eq!(binary_facets(&file), None);
        assert_eq!(binary_facets(&[0; 10]), None);
    }
}
