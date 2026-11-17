//! The versioned binary encoding of triangle meshes (CORE-MESH-002).
//!
//! An encoded mesh is the magic bytes `ARCM`, the version as a little-endian `u16`, then the
//! positions, triangles, channels and face groups. The byte layout is documented in
//! `docs/architecture/mesh-encoding.md`.
//!
//! - **Exact by default.** Without quantization every position (`f64`) and channel component
//!   (`f32`) is written bit for bit, and decoding returns a mesh equal to the encoded one. An
//!   exact encoding is unique: decoding it and encoding the result gives the same bytes.
//! - **Quantization on request.** [`Quantization::position_error`] stores positions on a grid
//!   over the bounding box: the encoder takes the smallest bit width from 1 to 32 at which
//!   every decoded position is within that distance of the original, measured on the decoded
//!   values. [`Quantization::normal_angle`] stores normals by octahedral mapping at the smallest
//!   bit width from 2 to 24 at which every decoded normal is within that angle of the original;
//!   decoded normals have unit length. When no bit width meets a bound (or there are no
//!   vertices, the extent of the box overflows, or a normal is zero), that part is stored
//!   exactly, which meets every bound. The bound and the bit width are stored.
//! - **Deterministic.** The same mesh and bounds always give the same bytes.
//! - **Bounded.** Decoding takes [`Limits`]: the input length is checked against `input_bytes`,
//!   every count against `array_length` and against the input that remains before anything is
//!   allocated, and names against `string_bytes`. The decoded parts go through
//!   [`TriMeshBuilder::build`], which validates every index against the vertex count and every
//!   group against the triangle count.
//!
//! Every decoding failure is `mesh.decode.invalid` with the details `offset` and `reason`; a
//! mesh that validation rejects also carries the validation code as `cause`.

use arc_base::id::Identifier;
use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, ObjectId, Result};

use crate::error::codes;
use crate::mesh::{
    AttributeKey, Channel, FaceGroup, MAX_CHANNELS, SubObjectKey, TriMesh, TriMeshBuilder,
};
use crate::{angle_between, length, unit};

/// The name and version of the mesh encoding, as a document descriptor names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshEncoding {
    /// The encoding's name.
    pub name: &'static str,
    /// The encoding's version; a decoder accepts only the version it implements.
    pub version: u16,
}

impl MeshEncoding {
    /// The encoding this crate writes and reads.
    pub const CURRENT: Self = Self {
        name: "arc.mesh",
        version: 1,
    };

    /// The four bytes every encoded mesh starts with.
    pub const MAGIC: [u8; 4] = *b"ARCM";
}

/// The error bounds of an encoding; `None` stores that part exactly.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Quantization {
    /// The largest distance between a position and its decoded value, in model units.
    pub position_error: Option<f64>,
    /// The largest angle between a normal and its decoded value, in radians.
    pub normal_angle: Option<f64>,
}

const KIND_NORMAL: u8 = 1;
const KIND_TANGENT: u8 = 2;
const KIND_UV: u8 = 3;
const KIND_COLOR: u8 = 4;
const KIND_USER: u8 = 5;

const MAX_POSITION_BITS: u8 = 32;
const MIN_NORMAL_BITS: u8 = 2;
const MAX_NORMAL_BITS: u8 = 24;

/// The bytes one quantized component of `bits` bits takes.
fn width(bits: u8) -> usize {
    usize::from(bits).div_ceil(8)
}

/// The largest code of `bits` bits, as a number.
fn steps(bits: u8) -> f64 {
    f64::from(u32::MAX >> (32 - u32::from(bits.clamp(1, 32))))
}

/// The largest octahedral code of `bits` bits: one less than the bit width allows, so the
/// number of steps is even and the coordinate axes are stored without error.
fn normal_steps(bits: u8) -> f64 {
    steps(bits) - 1.0
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is rounded and clamped to [0, steps] first, and steps is at most u32::MAX"
)]
fn code(fraction: f64, steps: f64) -> u32 {
    // NaN clamps to NaN and casts to 0.
    (fraction * steps).round().clamp(0.0, steps) as u32
}

/// The grid coordinate of `x` in `[min, max]`.
fn quantize(x: f64, min: f64, max: f64, steps: f64) -> u32 {
    if max > min {
        code((x - min) / (max - min), steps)
    } else {
        0
    }
}

/// The coordinate a grid code stands for.
fn dequantize(q: u32, min: f64, max: f64, steps: f64) -> f64 {
    min + (max - min) * (f64::from(q) / steps)
}

struct Grid {
    bits: u8,
    min: Point3,
    max: Point3,
    codes: Vec<[u32; 3]>,
}

fn grid_point(q: &[u32; 3], min: &Point3, max: &Point3, steps: f64) -> Point3 {
    Point3::new(
        dequantize(q[0], min.x, max.x, steps),
        dequantize(q[1], min.y, max.y, steps),
        dequantize(q[2], min.z, max.z, steps),
    )
}

/// The coarsest grid whose decoded positions are all within `bound`, if any.
fn quantize_positions(positions: &[Point3], bound: f64) -> Option<Grid> {
    let bbox = arc_geom::Aabb3::from_points(positions);
    if bbox.is_empty() {
        return None;
    }
    let (min, max) = (bbox.min(), bbox.max());
    let diagonal = length(&(max - min));
    if !diagonal.is_finite() {
        return None;
    }
    (1..=MAX_POSITION_BITS)
        // The grid's own error is half a cell diagonal; skip widths that cannot meet the bound.
        .filter(|&bits| diagonal / steps(bits) / 2.0 <= bound)
        .find_map(|bits| {
            let steps = steps(bits);
            let codes: Vec<[u32; 3]> = positions
                .iter()
                .map(|p| {
                    [
                        quantize(p.x, min.x, max.x, steps),
                        quantize(p.y, min.y, max.y, steps),
                        quantize(p.z, min.z, max.z, steps),
                    ]
                })
                .collect();
            let within = positions
                .iter()
                .zip(&codes)
                .all(|(p, q)| length(&(grid_point(q, &min, &max, steps) - p)) <= bound);
            within.then_some(Grid {
                bits,
                min,
                max,
                codes,
            })
        })
}

fn sign(x: f64) -> f64 {
    if x < 0.0 { -1.0 } else { 1.0 }
}

/// The octahedral codes of a unit direction.
fn octahedral(n: &Vector3, steps: f64) -> [u32; 2] {
    let p = n / (n.x.abs() + n.y.abs() + n.z.abs());
    let (u, v) = if p.z < 0.0 {
        ((1.0 - p.y.abs()) * sign(p.x), (1.0 - p.x.abs()) * sign(p.y))
    } else {
        (p.x, p.y)
    };
    [code((u + 1.0) / 2.0, steps), code((v + 1.0) / 2.0, steps)]
}

/// The unit direction octahedral codes stand for.
fn octahedral_direction(q: [u32; 2], steps: f64) -> [f32; 3] {
    let (u, v) = (
        f64::from(q[0]) / steps * 2.0 - 1.0,
        f64::from(q[1]) / steps * 2.0 - 1.0,
    );
    let z = 1.0 - u.abs() - v.abs();
    let folded = if z < 0.0 {
        Vector3::new((1.0 - v.abs()) * sign(u), (1.0 - u.abs()) * sign(v), z)
    } else {
        Vector3::new(u, v, z)
    };
    // |u| + |v| + |z| is at least 1, so the vector is never zero.
    let n = unit(&folded).unwrap_or_else(Vector3::z);
    arc_base::math::vector3_to_gpu(&n).to_array()
}

fn to_f64(v: &[f32; 3]) -> Vector3 {
    Vector3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
}

/// The smallest bit width whose decoded normals are all within `bound`, with the codes.
fn quantize_normals(normals: &[[f32; 3]], bound: f64) -> Option<(u8, Vec<[u32; 2]>)> {
    let units: Vec<Vector3> = normals
        .iter()
        .map(|n| unit(&to_f64(n)))
        .collect::<Option<_>>()?;
    (MIN_NORMAL_BITS..=MAX_NORMAL_BITS).find_map(|bits| {
        let steps = normal_steps(bits);
        let codes: Option<Vec<[u32; 2]>> = units
            .iter()
            .map(|n| {
                let q = octahedral(n, steps);
                let decoded = to_f64(&octahedral_direction(q, steps));
                (angle_between(n, &decoded) <= bound).then_some(q)
            })
            .collect();
        codes.map(|codes| (bits, codes))
    })
}

fn check_bound(bound: Option<f64>) -> Result<()> {
    if bound.is_none_or(|b| b.is_finite() && b > 0.0) {
        Ok(())
    } else {
        Err(Error::new(codes::QUANTIZATION_INVALID))
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, count: usize) {
        // usize is at most 64 bits on every supported target.
        self.0
            .extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&(value + 0.0).to_le_bytes());
    }

    fn point(&mut self, p: &Point3) {
        p.iter().for_each(|c| self.f64(*c));
    }

    fn quantized(&mut self, q: u32, bits: u8) {
        self.0.extend(q.to_le_bytes().into_iter().take(width(bits)));
    }
}

impl TriMesh {
    /// The encoding of this mesh, exact or quantized within the bounds of `quantization`.
    ///
    /// # Errors
    ///
    /// `mesh.quantization.invalid` when a bound is not a finite positive number.
    pub fn encode(&self, quantization: &Quantization) -> Result<Vec<u8>> {
        check_bound(quantization.position_error)?;
        check_bound(quantization.normal_angle)?;
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(&MeshEncoding::MAGIC);
        w.0.extend_from_slice(&MeshEncoding::CURRENT.version.to_le_bytes());

        let grid = quantization
            .position_error
            .and_then(|bound| Some((bound, quantize_positions(&self.positions, bound)?)));
        match &grid {
            Some((bound, grid)) => {
                w.u8(1);
                w.u8(grid.bits);
                w.f64(*bound);
                w.point(&grid.min);
                w.point(&grid.max);
                w.count(grid.codes.len());
                for q in grid.codes.iter().flatten() {
                    w.quantized(*q, grid.bits);
                }
            }
            None => {
                w.u8(0);
                w.count(self.positions.len());
                self.positions.iter().for_each(|p| w.point(p));
            }
        }

        w.count(self.triangles.len());
        self.triangles.iter().flatten().for_each(|i| w.u32(*i));

        w.count(self.channels.len());
        for (key, channel) in &self.channels {
            match key {
                AttributeKey::Normal => w.u8(KIND_NORMAL),
                AttributeKey::Tangent => w.u8(KIND_TANGENT),
                AttributeKey::Uv(set) => {
                    w.u8(KIND_UV);
                    w.u32(*set);
                }
                AttributeKey::Color => w.u8(KIND_COLOR),
                AttributeKey::User(name) => {
                    w.u8(KIND_USER);
                    w.count(name.len());
                    w.0.extend_from_slice(name.as_bytes());
                    w.u8(u8::try_from(channel.components()).unwrap_or(0));
                }
            }
            if let (AttributeKey::Normal, Channel::Vec3(normals)) = (key, channel) {
                let octahedral = quantization
                    .normal_angle
                    .and_then(|bound| Some((bound, quantize_normals(normals, bound)?)));
                if let Some((bound, (bits, codes))) = octahedral {
                    w.u8(1);
                    w.u8(bits);
                    w.f64(bound);
                    for q in codes.iter().flatten() {
                        w.quantized(*q, bits);
                    }
                    continue;
                }
                w.u8(0);
            }
            for c in channel.floats() {
                w.0.extend_from_slice(&(c + 0.0).to_le_bytes());
            }
        }

        w.count(self.groups.len());
        for g in &self.groups {
            w.u32(g.start);
            w.u32(g.end);
            w.u32(g.material);
            w.u32(g.key.local);
            w.u8(u8::from(g.key.object.is_some()));
            if let Some(object) = g.key.object {
                w.0.extend_from_slice(&object.as_u128().to_be_bytes());
            }
        }
        Ok(w.0)
    }

    /// Decodes a mesh that fills `bytes` exactly.
    ///
    /// # Errors
    ///
    /// `mesh.decode.invalid` for input that is malformed, truncated, exceeds `limits`, has bytes
    /// after the mesh, or describes a mesh that validation rejects.
    pub fn decode(bytes: &[u8], limits: &Limits) -> Result<TriMesh> {
        if !u64::try_from(bytes.len()).is_ok_and(|len| len <= limits.input_bytes) {
            return Err(invalid(0, "input exceeds the input byte limit"));
        }
        let mut r = Reader {
            bytes,
            pos: 0,
            limits,
        };
        if r.take::<4>()? != MeshEncoding::MAGIC {
            return Err(invalid(0, "not a mesh encoding"));
        }
        if u16::from_le_bytes(r.take()?) != MeshEncoding::CURRENT.version {
            return Err(invalid(4, "unsupported encoding version"));
        }

        let positions = r.positions()?;
        let vertices = positions.len();

        let count = r.count(12)?;
        let mut triangles = Vec::with_capacity(count);
        for _ in 0..count {
            triangles.push([r.u32()?, r.u32()?, r.u32()?]);
        }
        let mut builder = TriMeshBuilder::new(positions, triangles);

        builder = r.channels(builder, vertices)?;

        let count = r.count(17)?;
        for _ in 0..count {
            let (start, end, material, local) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            let object = if r.flag()? {
                Some(ObjectId::from_u128(u128::from_be_bytes(r.take()?)))
            } else {
                None
            };
            builder = builder.group(FaceGroup {
                start,
                end,
                material,
                key: SubObjectKey { object, local },
            });
        }
        if r.pos != bytes.len() {
            return Err(invalid(r.pos, "trailing bytes after the mesh"));
        }
        builder.build(limits).map_err(|cause| {
            invalid(r.pos, "the mesh is not valid")
                .with_detail("cause", cause.code())
                .with_source(cause)
        })
    }
}

fn invalid(at: usize, reason: &'static str) -> Error {
    Error::new(codes::DECODE_INVALID)
        .with_detail("offset", i64::try_from(at).unwrap_or(i64::MAX))
        .with_detail("reason", reason)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    limits: &'a Limits,
}

impl Reader<'_> {
    fn slice(&mut self, len: usize) -> Result<&[u8]> {
        let slice = self
            .pos
            .checked_add(len)
            .and_then(|end| self.bytes.get(self.pos..end))
            .ok_or_else(|| invalid(self.pos, "unexpected end of input"))?;
        self.pos += len;
        Ok(slice)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let at = self.pos;
        <[u8; N]>::try_from(self.slice(N)?).map_err(|_| invalid(at, "unexpected end of input"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(u8::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn flag(&mut self) -> Result<bool> {
        let at = self.pos;
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid(at, "flag is neither 0 nor 1")),
        }
    }

    /// A one-byte number in `min..=max`.
    fn bits(&mut self, min: u8, max: u8) -> Result<u8> {
        let at = self.pos;
        let bits = self.u8()?;
        if (min..=max).contains(&bits) {
            Ok(bits)
        } else {
            Err(invalid(at, "bit width or component count out of range"))
        }
    }

    fn f64(&mut self) -> Result<f64> {
        let at = self.pos;
        let value = f64::from_le_bytes(self.take()?);
        if !value.is_finite() {
            return Err(invalid(at, "number is not finite"));
        }
        if value == 0.0 && value.is_sign_negative() {
            return Err(invalid(at, "negative zero"));
        }
        Ok(value)
    }

    /// A stored error bound: finite and positive.
    fn bound(&mut self) -> Result<f64> {
        let at = self.pos;
        let bound = self.f64()?;
        if bound > 0.0 {
            Ok(bound)
        } else {
            Err(invalid(at, "error bound is not positive"))
        }
    }

    fn point(&mut self) -> Result<Point3> {
        Ok(Point3::new(self.f64()?, self.f64()?, self.f64()?))
    }

    /// A quantized component stored in the bytes of `bits` bits, at most `steps`.
    fn quantized(&mut self, bits: u8, steps: f64) -> Result<u32> {
        let at = self.pos;
        let mut le = [0_u8; 4];
        for (out, byte) in le.iter_mut().zip(self.slice(width(bits))?) {
            *out = *byte;
        }
        let q = u32::from_le_bytes(le);
        if f64::from(q) <= steps {
            Ok(q)
        } else {
            Err(invalid(at, "quantized value exceeds its range"))
        }
    }

    /// Fails unless `count` elements of `size` bytes fit in the remaining input.
    fn fits(&self, count: usize, size: usize) -> Result<()> {
        if count
            .checked_mul(size)
            .is_some_and(|needed| needed <= self.bytes.len() - self.pos)
        {
            Ok(())
        } else {
            Err(invalid(self.pos, "count exceeds the remaining input"))
        }
    }

    /// The positions section: exact, or on a grid over a stored box.
    fn positions(&mut self) -> Result<Vec<Point3>> {
        Ok(if self.flag()? {
            let bits = self.bits(1, MAX_POSITION_BITS)?;
            self.bound()?;
            let at = self.pos;
            let (min, max) = (self.point()?, self.point()?);
            let ordered = min.iter().zip(max.iter()).all(|(low, high)| low <= high);
            if !ordered || !(max - min).iter().all(|c| c.is_finite()) {
                return Err(invalid(at, "invalid quantization box"));
            }
            let steps = steps(bits);
            let count = self.count(3 * width(bits))?;
            let mut positions = Vec::with_capacity(count);
            for _ in 0..count {
                let q = [
                    self.quantized(bits, steps)?,
                    self.quantized(bits, steps)?,
                    self.quantized(bits, steps)?,
                ];
                positions.push(grid_point(&q, &min, &max, steps));
            }
            positions
        } else {
            let count = self.count(24)?;
            let mut positions = Vec::with_capacity(count);
            for _ in 0..count {
                positions.push(self.point()?);
            }
            positions
        })
    }

    /// The channels section, added to `builder` in key order.
    fn channels(&mut self, mut builder: TriMeshBuilder, vertices: usize) -> Result<TriMeshBuilder> {
        let at = self.pos;
        let count = self.count(1)?;
        if count > MAX_CHANNELS {
            return Err(invalid(at, "too many channels"));
        }
        let mut previous: Option<AttributeKey> = None;
        for _ in 0..count {
            let at = self.pos;
            let (key, components) = match self.u8()? {
                KIND_NORMAL => (AttributeKey::Normal, 3),
                KIND_TANGENT => (AttributeKey::Tangent, 4),
                KIND_UV => (AttributeKey::Uv(self.u32()?), 2),
                KIND_COLOR => (AttributeKey::Color, 4),
                KIND_USER => {
                    let name = self.name()?;
                    (AttributeKey::User(name), self.bits(1, 4)?)
                }
                _ => return Err(invalid(at, "unknown channel kind")),
            };
            if previous.as_ref().is_some_and(|p| *p >= key) {
                return Err(invalid(at, "channels are not in key order"));
            }
            let channel = if key == AttributeKey::Normal && self.flag()? {
                let bits = self.bits(MIN_NORMAL_BITS, MAX_NORMAL_BITS)?;
                self.bound()?;
                self.fits(vertices, 2 * width(bits))?;
                let steps = normal_steps(bits);
                let mut normals = Vec::with_capacity(vertices);
                for _ in 0..vertices {
                    let q = [self.quantized(bits, steps)?, self.quantized(bits, steps)?];
                    normals.push(octahedral_direction(q, steps));
                }
                Channel::Vec3(normals)
            } else {
                self.channel(vertices, components)?
            };
            builder = builder.channel(key.clone(), channel);
            previous = Some(key);
        }
        Ok(builder)
    }

    /// A count of elements whose encoding takes at least `min_size` bytes each.
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let at = self.pos;
        let count = u64::from_le_bytes(self.take()?);
        if count > self.limits.array_length {
            return Err(invalid(at, "count exceeds the array length limit"));
        }
        let count =
            usize::try_from(count).map_err(|_| invalid(at, "count exceeds the address space"))?;
        self.fits(count, min_size)
            .map_err(|_| invalid(at, "count exceeds the remaining input"))?;
        Ok(count)
    }

    fn name(&mut self) -> Result<String> {
        let at = self.pos;
        let len = u64::from_le_bytes(self.take()?);
        if len > self.limits.string_bytes {
            return Err(invalid(at, "name exceeds the string length limit"));
        }
        let len = usize::try_from(len).map_err(|_| invalid(at, "unexpected end of input"))?;
        std::str::from_utf8(self.slice(len)?)
            .map(str::to_owned)
            .map_err(|_| invalid(at, "name is not UTF-8"))
    }

    fn f32(&mut self) -> Result<f32> {
        let at = self.pos;
        let value = f32::from_le_bytes(self.take()?);
        if !value.is_finite() {
            return Err(invalid(at, "number is not finite"));
        }
        if value == 0.0 && value.is_sign_negative() {
            return Err(invalid(at, "negative zero"));
        }
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut value = [0.0; N];
        for c in &mut value {
            *c = self.f32()?;
        }
        Ok(value)
    }

    fn values<const N: usize>(&mut self, count: usize) -> Result<Vec<[f32; N]>> {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.array()?);
        }
        Ok(values)
    }

    /// An exact channel of `count` values of `components` components.
    fn channel(&mut self, count: usize, components: u8) -> Result<Channel> {
        self.fits(count, 4 * usize::from(components))?;
        Ok(match components {
            1 => Channel::Scalar(self.values::<1>(count)?.into_iter().map(|[c]| c).collect()),
            2 => Channel::Vec2(self.values(count)?),
            3 => Channel::Vec3(self.values(count)?),
            _ => Channel::Vec4(self.values(count)?),
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test values are small and exact enough"
)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::NormalWeighting;
    use crate::testing::{cube, mesh, sphere};

    fn reason(error: &Error) -> String {
        assert_eq!(error.code(), "mesh.decode.invalid");
        match error.details().get("reason") {
            Some(arc_base::error::Value::Text(text)) => text.clone(),
            other => panic!("no reason: {other:?}"),
        }
    }

    fn exact(mesh: &TriMesh) -> Vec<u8> {
        mesh.encode(&Quantization::default()).unwrap()
    }

    fn with_normals(mesh: &TriMesh) -> TriMesh {
        let exact = arc_base::Tolerance {
            linear: 0.0,
            ..arc_base::Tolerance::DEFAULT
        };
        mesh.with_normals(NormalWeighting::Angle, 1.0, &exact, &Limits::DEFAULT)
            .unwrap()
    }

    proptest! {
        /// Without quantization, decoding returns the mesh bit for bit and the bytes are unique.
        #[test]
        fn exact_encoding_round_trips(mesh in mesh()) {
            let bytes = exact(&mesh);
            let decoded = TriMesh::decode(&bytes, &Limits::STRICT).unwrap();
            prop_assert_eq!(&decoded, &mesh);
            for (a, b) in decoded.positions().iter().zip(mesh.positions()) {
                prop_assert_eq!(a.coords.map(f64::to_bits), b.coords.map(f64::to_bits));
            }
            prop_assert_eq!(exact(&decoded), bytes);
        }

        /// With quantization, every decoded position and normal is within the stated bound,
        /// measured on the decoded mesh; everything else is unchanged.
        #[test]
        fn quantized_encoding_stays_within_its_bounds(
            mesh in mesh(),
            position_exponent in -12.0..2.0_f64,
            normal_angle in 1e-6..0.5_f64,
        ) {
            let mesh = with_normals(&mesh);
            let diagonal = {
                let bbox = mesh.bounding_box();
                length(&(bbox.max() - bbox.min()))
            };
            // A bound relative to the size of the mesh; an overflowing or empty extent falls
            // back to exact storage, which meets any bound.
            let position_error = if diagonal.is_finite() && diagonal > 0.0 {
                diagonal * 10.0_f64.powf(position_exponent)
            } else {
                1.0
            };
            prop_assume!(position_error.is_finite() && position_error > 0.0);
            let quantization = Quantization {
                position_error: Some(position_error),
                normal_angle: Some(normal_angle),
            };
            let bytes = mesh.encode(&quantization).unwrap();
            prop_assert_eq!(&mesh.encode(&quantization).unwrap(), &bytes);
            let decoded = TriMesh::decode(&bytes, &Limits::STRICT).unwrap();
            for (a, b) in decoded.positions().iter().zip(mesh.positions()) {
                prop_assert!(length(&(a - b)) <= position_error);
            }
            for (a, b) in decoded.normals().unwrap().iter().zip(mesh.normals().unwrap()) {
                prop_assert!(angle_between(&to_f64(a), &to_f64(b)) <= normal_angle);
                prop_assert!((to_f64(a).norm() - 1.0).abs() < 1e-6);
            }
            prop_assert_eq!(decoded.triangles(), mesh.triangles());
            prop_assert_eq!(decoded.groups(), mesh.groups());
            prop_assert_eq!(decoded.uv_set(3), mesh.uv_set(3));
        }

        /// Changing, removing or adding bytes never panics: the result is a valid mesh or
        /// `mesh.decode.invalid`. A changed exact encoding that still decodes encodes back to
        /// exactly the changed bytes.
        #[test]
        fn damaged_encodings_are_rejected_or_valid(
            mesh in mesh(),
            at in any::<prop::sample::Index>(),
            xor in 1..=255_u8,
            quantized in any::<bool>(),
        ) {
            let quantization = if quantized {
                Quantization { position_error: Some(1e-3), normal_angle: Some(1e-2) }
            } else {
                Quantization::default()
            };
            let mut bytes = with_normals(&mesh).encode(&quantization).unwrap();
            let i = at.index(bytes.len());
            for cut in [i, bytes.len() - 1] {
                let error = TriMesh::decode(&bytes[..cut], &Limits::STRICT).unwrap_err();
                prop_assert_eq!(error.code(), "mesh.decode.invalid");
            }
            bytes[i] ^= xor;
            match TriMesh::decode(&bytes, &Limits::STRICT) {
                Ok(decoded) => {
                    if !quantized {
                        prop_assert_eq!(exact(&decoded), bytes.clone());
                    }
                }
                Err(error) => prop_assert_eq!(error.code(), "mesh.decode.invalid"),
            }
            bytes.push(0);
            prop_assert!(TriMesh::decode(&bytes, &Limits::STRICT).is_err());
        }

        #[test]
        fn arbitrary_bytes_never_panic(body in prop::collection::vec(any::<u8>(), 0..256)) {
            let mut bytes = b"ARCM\x01\x00".to_vec();
            bytes.extend_from_slice(&body);
            for input in [&body, &bytes] {
                if let Err(error) = TriMesh::decode(input, &Limits::STRICT) {
                    prop_assert_eq!(error.code(), "mesh.decode.invalid");
                }
            }
        }
    }

    #[test]
    fn quantization_picks_the_coarsest_grid_that_meets_the_bound() {
        let sphere = with_normals(&sphere(Point3::new(10.0, 20.0, 30.0), 2.0, 12, 24));
        let exact_len = exact(&sphere).len();
        let coarse = Quantization {
            position_error: Some(1e-2),
            normal_angle: Some(1e-2),
        };
        let bytes = sphere.encode(&coarse).unwrap();
        assert!(bytes.len() < exact_len * 3 / 4);
        // Mode 1 and a bit width: the diagonal 4√3 over 2 · 0.01 needs 9 bits.
        assert_eq!(&bytes[6..8], &[1, 9]);
        // A bound below what 32 bits can give stays exact.
        let fine = Quantization {
            position_error: Some(1e-30),
            normal_angle: Some(1e-12),
        };
        let bytes = sphere.encode(&fine).unwrap();
        assert_eq!(bytes.len(), exact_len);
        assert_eq!(TriMesh::decode(&bytes, &Limits::STRICT).unwrap(), sphere);
        // A single point and an empty mesh quantize trivially or not at all.
        let point =
            TriMesh::new(vec![Point3::new(1.0, 2.0, 3.0)], vec![], &Limits::STRICT).unwrap();
        let bytes = point.encode(&coarse).unwrap();
        assert_eq!(TriMesh::decode(&bytes, &Limits::STRICT).unwrap(), point);
        let empty = TriMesh::default();
        let bytes = empty.encode(&coarse).unwrap();
        assert_eq!(bytes[6], 0);
        assert_eq!(TriMesh::decode(&bytes, &Limits::STRICT).unwrap(), empty);
    }

    #[test]
    fn invalid_bounds_are_rejected() {
        for bound in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for quantization in [
                Quantization {
                    position_error: Some(bound),
                    normal_angle: None,
                },
                Quantization {
                    position_error: None,
                    normal_angle: Some(bound),
                },
            ] {
                assert_eq!(
                    cube().encode(&quantization).unwrap_err().code(),
                    "mesh.quantization.invalid"
                );
            }
        }
    }

    #[test]
    fn header_indices_and_groups_are_checked() {
        let limits = Limits::STRICT;
        let bytes = exact(&cube());
        let decode = |bytes: &[u8]| reason(&TriMesh::decode(bytes, &limits).unwrap_err());
        assert_eq!(decode(b"ARC"), "unexpected end of input");
        let mut wrong = bytes.clone();
        wrong[3] = b'G';
        assert_eq!(decode(&wrong), "not a mesh encoding");
        let mut wrong = bytes.clone();
        wrong[4] = 2;
        assert_eq!(decode(&wrong), "unsupported encoding version");
        let mut wrong = bytes.clone();
        wrong[6] = 2;
        assert_eq!(decode(&wrong), "flag is neither 0 nor 1");
        // The first index of the first triangle: 6 header, 1 mode, 8 count, 8 × 24 positions,
        // 8 count.
        let mut wrong = bytes.clone();
        wrong[6 + 1 + 8 + 192 + 8] = 8;
        let error = TriMesh::decode(&wrong, &limits).unwrap_err();
        assert_eq!(reason(&error), "the mesh is not valid");
        assert_eq!(
            error.details().get("cause"),
            Some(&arc_base::error::Value::Text(
                "mesh.index.out_of_range".into()
            ))
        );
        // A group past the last triangle.
        let grouped = cube()
            .into_builder()
            .group(FaceGroup {
                start: 0,
                end: 12,
                material: 0,
                key: SubObjectKey {
                    object: None,
                    local: 0,
                },
            })
            .build(&limits)
            .unwrap();
        let mut wrong = exact(&grouped);
        let end = wrong.len() - 13;
        wrong[end] = 13;
        let error = TriMesh::decode(&wrong, &limits).unwrap_err();
        assert_eq!(
            error.details().get("cause"),
            Some(&arc_base::error::Value::Text("mesh.group.invalid".into()))
        );
    }

    #[test]
    fn limits_are_enforced_before_allocating() {
        let mut bytes = b"ARCM\x01\x00\x00".to_vec();
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());
        let decode =
            |bytes: &[u8], limits: &Limits| reason(&TriMesh::decode(bytes, limits).unwrap_err());
        assert_eq!(
            decode(&bytes, &Limits::STRICT),
            "count exceeds the array length limit"
        );
        let mut bytes = b"ARCM\x01\x00\x00".to_vec();
        bytes.extend_from_slice(&1000_u64.to_le_bytes());
        assert_eq!(
            decode(&bytes, &Limits::STRICT),
            "count exceeds the remaining input"
        );
        let cube_bytes = exact(&cube());
        let tight = Limits {
            input_bytes: 10,
            ..Limits::STRICT
        };
        assert_eq!(
            decode(&cube_bytes, &tight),
            "input exceeds the input byte limit"
        );
        let tight = Limits {
            array_length: 7,
            ..Limits::STRICT
        };
        assert_eq!(
            decode(&cube_bytes, &tight),
            "count exceeds the array length limit"
        );
        let named = cube()
            .into_builder()
            .channel(
                AttributeKey::User("name".into()),
                Channel::Scalar(vec![0.0; 8]),
            )
            .build(&Limits::STRICT)
            .unwrap();
        let tight = Limits {
            string_bytes: 3,
            ..Limits::STRICT
        };
        assert_eq!(
            decode(&exact(&named), &tight),
            "name exceeds the string length limit"
        );
    }

    #[test]
    fn non_canonical_fields_are_rejected() {
        let limits = Limits::STRICT;
        let mesh = cube()
            .into_builder()
            .channel(
                AttributeKey::Normal,
                Channel::Vec3(vec![[0.0, 0.0, 1.0]; 8]),
            )
            .channel(AttributeKey::Uv(0), Channel::Vec2(vec![[0.0; 2]; 8]))
            .channel(AttributeKey::Uv(1), Channel::Vec2(vec![[0.0; 2]; 8]))
            .build(&limits)
            .unwrap();
        let bytes = exact(&mesh);
        let decode = |bytes: &[u8]| reason(&TriMesh::decode(bytes, &limits).unwrap_err());
        // Negative zero in the first position.
        let mut wrong = bytes.clone();
        wrong[6 + 1 + 8 + 7] = 0x80;
        assert_eq!(decode(&wrong), "negative zero");
        // Swapping the indices of the two UV sets breaks the key order.
        let channels = 6 + 1 + 8 + 192 + 8 + 144 + 8;
        let uv0 = channels + 1 + 1 + 96;
        assert_eq!(bytes[uv0], KIND_UV);
        let mut wrong = bytes.clone();
        wrong[uv0 + 1] = 1;
        assert_eq!(decode(&wrong), "channels are not in key order");
        let mut wrong = bytes.clone();
        wrong[uv0] = 9;
        assert_eq!(decode(&wrong), "unknown channel kind");
        // A quantized value above its bit width.
        let quantized = mesh
            .encode(&Quantization {
                position_error: Some(0.3),
                normal_angle: None,
            })
            .unwrap();
        assert_eq!(&quantized[6..8], &[1, 2]);
        let mut wrong = quantized.clone();
        wrong[8 + 8 + 48 + 8] = 4;
        assert_eq!(decode(&wrong), "quantized value exceeds its range");
        let mut wrong = quantized;
        wrong[7] = 33;
        assert_eq!(decode(&wrong), "bit width or component count out of range");
    }
}
