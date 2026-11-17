//! The versioned canonical byte encoding of geometry values (CORE-GEOM-032).
//!
//! Every stored geometry value has one byte form, written by hand and independent of any
//! serialization library: a header of the four magic bytes `ARCG` and the encoding version as a
//! little-endian `u16`, then the value. A value is a one-byte kind tag followed by its fields in
//! a fixed order: numbers as little-endian IEEE 754 binary64, counts as little-endian `u64`,
//! flags as one byte 0 or 1, triangle indices as little-endian `u32`. Values inside other values
//! (the curves of a loop, the loops of a region) are complete tagged values without a header.
//! The byte layout of every kind is documented in `docs/architecture/geometry-encoding.md`.
//!
//! - **Canonical.** Equal values give equal bytes: the sign of zero is not part of a value, so
//!   −0.0 is written as +0.0, and every other number is written bit for bit. Decoding a value
//!   and encoding it again gives the same value; the arc of a polyline segment
//!   ([`crate::Polyline2::segment`]) loses the chord it keeps for evaluation, as with serde.
//! - **Unique.** The decoder accepts only the canonical form: it builds every value with the
//!   type's validating constructor, encodes the result again and rejects the input unless the
//!   bytes are identical, so input that a constructor would normalize (an arc start angle
//!   outside [0, 2π), a bulge on the last vertex of an open polyline, a region loop with the
//!   wrong orientation) is rejected.
//! - **Bounded.** Decoding takes [`Limits`]: the input length is checked against
//!   `input_bytes`, every count against `array_length` and against the input that remains
//!   before anything is allocated, and the nesting of values (at most three levels: region,
//!   loop, curve) against `nesting_depth`. Nothing recurses beyond that depth.
//!
//! Every failure is `geom.decode.invalid` with the details `offset` (the byte position) and
//! `reason`; a value that its constructor rejects also carries the constructor's code as
//! `cause` and the constructor's error as the source.

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::{
    Aabb2, Aabb3, Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, Line2, Loop2, Polyline2,
    PolylineVertex, Projection, Ray2, Region, RegionPart, Segment2, TriangleMesh2, ViewFrame,
};

/// The name and version of a geometry encoding, as a document descriptor names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GeomEncoding {
    /// The encoding's name.
    pub name: &'static str,
    /// The encoding's version; a decoder accepts only the version it implements.
    pub version: u16,
}

impl GeomEncoding {
    /// The encoding this crate writes and reads.
    pub const CURRENT: Self = Self {
        name: "arc.geom",
        version: 1,
    };

    /// The four bytes every encoded value starts with.
    pub const MAGIC: [u8; 4] = *b"ARCG";
}

// Kind tags. 0x18-0x1F are reserved for further 2D curve kinds (NURBS, spirals, composites),
// 0x60-0x6F for 3D curves, 0x70-0x7F for surfaces; every unassigned tag is rejected.
const SEGMENT: u8 = 0x10;
const RAY: u8 = 0x11;
const LINE: u8 = 0x12;
const CIRCLE: u8 = 0x13;
const ARC: u8 = 0x14;
const ELLIPSE: u8 = 0x15;
const ELLIPTICAL_ARC: u8 = 0x16;
const POLYLINE: u8 = 0x17;
const LOOP: u8 = 0x20;
const REGION: u8 = 0x21;
const AABB2: u8 = 0x30;
const AABB3: u8 = 0x31;
const VIEW_FRAME: u8 = 0x40;
const TRIANGLE_MESH: u8 = 0x50;

// The smallest encoded size of an element of each counted array, used to reject a count that
// the remaining input cannot hold before allocating.
const MIN_VERTEX: u64 = 40;
const MIN_POINT: u64 = 16;
const MIN_TRIANGLE: u64 = 12;
/// A circle: tag and three numbers.
const MIN_CURVE: u64 = 1 + 24;
/// A loop of one circle: tag, count and curve.
const MIN_LOOP: u64 = 1 + 8 + MIN_CURVE;
/// A part without holes: outer loop and hole count.
const MIN_PART: u64 = MIN_LOOP + 8;

/// Encoding and decoding in the canonical byte form, implemented by every stored geometry type.
///
/// A [`Curve2`] encodes exactly as the curve type it holds, so the bytes of a [`Segment2`] are
/// those of `Curve2::Segment` of it, and [`Curve2::decode`] reads any curve kind.
///
/// ```
/// use arc_base::Limits;
/// use arc_base::math::Point2;
/// use arc_geom::encoding::GeomCodec;
/// use arc_geom::{Curve2, Segment2};
///
/// let segment = Segment2::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)).unwrap();
/// let bytes = segment.encode();
/// assert_eq!(bytes.len(), segment.encoded_len());
/// assert_eq!(Segment2::decode(&bytes, &Limits::STRICT).unwrap(), segment);
/// assert_eq!(Curve2::decode(&bytes, &Limits::STRICT).unwrap(), Curve2::Segment(segment));
/// ```
pub trait GeomCodec: sealed::Value {
    /// The canonical encoding: header and value.
    #[must_use]
    fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(self.encoded_len());
        writer.header();
        self.write(&mut writer);
        writer.bytes
    }

    /// The length of [`Self::encode`] in bytes, computed without allocating.
    #[must_use]
    fn encoded_len(&self) -> usize {
        let mut writer = Writer::counter();
        writer.header();
        self.write(&mut writer);
        writer.len
    }

    /// Decodes a value that fills `bytes` exactly.
    ///
    /// # Errors
    ///
    /// `geom.decode.invalid` for input that is malformed, not canonical, exceeds `limits`, or
    /// has bytes after the value.
    fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let (value, rest) = Self::decode_prefix(bytes, limits)?;
        if rest.is_empty() {
            Ok(value)
        } else {
            Err(invalid(
                bytes.len() - rest.len(),
                "trailing bytes after the value",
            ))
        }
    }

    /// Decodes the value at the start of `bytes` and returns it with the bytes after it.
    ///
    /// # Errors
    ///
    /// `geom.decode.invalid` for input that is malformed, not canonical or exceeds `limits`.
    fn decode_prefix<'a>(bytes: &'a [u8], limits: &Limits) -> Result<(Self, &'a [u8])> {
        if !u64::try_from(bytes.len()).is_ok_and(|len| len <= limits.input_bytes) {
            return Err(invalid(0, "input exceeds the input byte limit"));
        }
        let mut reader = Reader {
            bytes,
            pos: 0,
            limits,
            depth: 0,
        };
        if reader.take::<4>()? != GeomEncoding::MAGIC {
            return Err(invalid(0, "not a geometry encoding"));
        }
        if u16::from_le_bytes(reader.take()?) != GeomEncoding::CURRENT.version {
            return Err(invalid(4, "unsupported encoding version"));
        }
        let value = reader.nested::<Self>()?;
        let (consumed, rest) = bytes.split_at_checked(reader.pos).unwrap_or((bytes, &[]));
        let canonical = value.encode();
        if canonical != consumed {
            let at = canonical
                .iter()
                .zip(consumed)
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| canonical.len().min(consumed.len()));
            return Err(invalid(at, "not the canonical encoding of the value"));
        }
        Ok((value, rest))
    }
}

fn invalid(at: usize, reason: &'static str) -> Error {
    Error::new(codes::DECODE_INVALID)
        .with_detail("offset", i64::try_from(at).unwrap_or(i64::MAX))
        .with_detail("reason", reason)
}

mod sealed {
    use arc_base::{Limits, Result};

    /// Writes bytes, or only counts them.
    pub struct Writer {
        pub(super) bytes: Vec<u8>,
        pub(super) len: usize,
        pub(super) count_only: bool,
    }

    /// Reads bytes within the limits.
    pub struct Reader<'a> {
        pub(super) bytes: &'a [u8],
        pub(super) pos: usize,
        pub(super) limits: &'a Limits,
        pub(super) depth: u32,
    }

    /// A value with a kind tag and a body; not implementable outside this crate.
    pub trait Value: Sized {
        /// Writes the tag and the body.
        fn write(&self, writer: &mut Writer);
        /// Reads the tag and the body.
        fn read(reader: &mut Reader<'_>) -> Result<Self>;
    }

    /// A value of one kind tag.
    pub trait Body: Sized {
        const TAG: u8;
        fn write_body(&self, writer: &mut Writer);
        fn read_body(reader: &mut Reader<'_>) -> Result<Self>;
    }

    impl<T: Body> Value for T {
        fn write(&self, writer: &mut Writer) {
            writer.u8(T::TAG);
            self.write_body(writer);
        }

        fn read(reader: &mut Reader<'_>) -> Result<Self> {
            let at = reader.pos;
            if reader.u8()? != T::TAG {
                return Err(super::invalid(at, "unexpected kind tag"));
            }
            T::read_body(reader)
        }
    }
}

use sealed::{Body, Reader, Value, Writer};

impl Writer {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            len: 0,
            count_only: false,
        }
    }

    fn counter() -> Self {
        Self {
            bytes: Vec::new(),
            len: 0,
            count_only: true,
        }
    }

    fn put(&mut self, bytes: &[u8]) {
        self.len = self.len.saturating_add(bytes.len());
        if !self.count_only {
            self.bytes.extend_from_slice(bytes);
        }
    }

    fn header(&mut self) {
        self.put(&GeomEncoding::MAGIC);
        self.put(&GeomEncoding::CURRENT.version.to_le_bytes());
    }

    fn u8(&mut self, value: u8) {
        self.put(&[value]);
    }

    fn flag(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn count(&mut self, count: usize) {
        // usize is at most 64 bits on every supported target.
        self.put(&u64::try_from(count).unwrap_or(u64::MAX).to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        // The sign of zero is not part of a value.
        let value = if value == 0.0 { 0.0 } else { value };
        self.put(&value.to_le_bytes());
    }

    fn point2(&mut self, p: &Point2) {
        self.f64(p.x);
        self.f64(p.y);
    }

    fn vector2(&mut self, v: &Vector2) {
        self.f64(v.x);
        self.f64(v.y);
    }

    fn point3(&mut self, p: &Point3) {
        self.f64(p.x);
        self.f64(p.y);
        self.f64(p.z);
    }
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self
            .pos
            .checked_add(N)
            .and_then(|end| self.bytes.get(self.pos..end))
            .and_then(|slice| <[u8; N]>::try_from(slice).ok())
            .ok_or_else(|| invalid(self.pos, "unexpected end of input"))?;
        self.pos += N;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }

    fn peek(&self) -> Result<u8> {
        self.bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| invalid(self.pos, "unexpected end of input"))
    }

    fn flag(&mut self) -> Result<bool> {
        let at = self.pos;
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid(at, "flag is neither 0 nor 1")),
        }
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
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

    fn point2(&mut self) -> Result<Point2> {
        Ok(Point2::new(self.f64()?, self.f64()?))
    }

    fn vector2(&mut self) -> Result<Vector2> {
        Ok(Vector2::new(self.f64()?, self.f64()?))
    }

    fn point3(&mut self) -> Result<Point3> {
        Ok(Point3::new(self.f64()?, self.f64()?, self.f64()?))
    }

    fn vector3(&mut self) -> Result<Vector3> {
        Ok(Vector3::new(self.f64()?, self.f64()?, self.f64()?))
    }

    /// A count of elements whose encoding takes at least `min_size` bytes each.
    fn count(&mut self, min_size: u64) -> Result<usize> {
        let at = self.pos;
        let count = u64::from_le_bytes(self.take()?);
        if count > self.limits.array_length {
            return Err(invalid(at, "count exceeds the array length limit"));
        }
        let remaining = u64::try_from(self.bytes.len() - self.pos).unwrap_or(u64::MAX);
        if count
            .checked_mul(min_size)
            .is_none_or(|needed| needed > remaining)
        {
            return Err(invalid(at, "count exceeds the remaining input"));
        }
        usize::try_from(count).map_err(|_| invalid(at, "count exceeds the address space"))
    }

    /// Reads a value one nesting level deeper.
    fn nested<T: Value>(&mut self) -> Result<T> {
        if self.depth >= self.limits.nesting_depth {
            return Err(invalid(self.pos, "value exceeds the nesting depth limit"));
        }
        self.depth += 1;
        let value = T::read(self);
        self.depth -= 1;
        value
    }

    /// Reads `count` values one nesting level deeper.
    fn nested_values<T: Value>(&mut self, count: usize) -> Result<Vec<T>> {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.nested()?);
        }
        Ok(values)
    }
}

/// Maps a constructor's error at byte `at` to `geom.decode.invalid`.
fn constructed<T>(at: usize, result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        invalid(at, "the value's constructor rejects it")
            .with_detail("cause", error.code())
            .with_source(error)
    })
}

/// The only call that depends on the signature of [`ViewFrame::new`]; deserialization uses the
/// same default tolerance.
fn view_frame(
    eye: Point3,
    target: Point3,
    up: Vector3,
    projection: Projection,
    near: f64,
    far: f64,
) -> Result<ViewFrame> {
    ViewFrame::new(eye, target, up, projection, near, far)
}

impl Body for Segment2 {
    const TAG: u8 = SEGMENT;

    fn write_body(&self, w: &mut Writer) {
        w.point2(&self.start());
        w.point2(&self.end());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (start, end) = (r.point2()?, r.point2()?);
        constructed(at, Self::new(start, end))
    }
}

impl Body for Ray2 {
    const TAG: u8 = RAY;

    fn write_body(&self, w: &mut Writer) {
        w.point2(&self.origin());
        w.vector2(&self.direction());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (origin, direction) = (r.point2()?, r.vector2()?);
        constructed(at, Self::new(origin, direction))
    }
}

impl Body for Line2 {
    const TAG: u8 = LINE;

    fn write_body(&self, w: &mut Writer) {
        w.point2(&self.point());
        w.vector2(&self.direction());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (point, direction) = (r.point2()?, r.vector2()?);
        constructed(at, Self::new(point, direction))
    }
}

impl Body for Circle2 {
    const TAG: u8 = CIRCLE;

    fn write_body(&self, w: &mut Writer) {
        w.point2(&self.center());
        w.f64(self.radius());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (center, radius) = (r.point2()?, r.f64()?);
        constructed(at, Self::new(center, radius))
    }
}

impl Body for Arc2 {
    const TAG: u8 = ARC;

    fn write_body(&self, w: &mut Writer) {
        w.point2(&self.center());
        w.f64(self.radius());
        w.f64(self.start_angle());
        w.f64(self.sweep());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (center, radius, start, sweep) = (r.point2()?, r.f64()?, r.f64()?, r.f64()?);
        constructed(at, Self::new(center, radius, start, sweep))
    }
}

fn write_ellipse(w: &mut Writer, ellipse: &Ellipse2) {
    w.point2(&ellipse.center());
    w.vector2(&ellipse.major_axis());
    w.f64(ellipse.ratio());
}

fn read_ellipse(r: &mut Reader<'_>) -> Result<Ellipse2> {
    let at = r.pos;
    let (center, major_axis, ratio) = (r.point2()?, r.vector2()?, r.f64()?);
    constructed(at, Ellipse2::new(center, major_axis, ratio))
}

impl Body for Ellipse2 {
    const TAG: u8 = ELLIPSE;

    fn write_body(&self, w: &mut Writer) {
        write_ellipse(w, self);
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        read_ellipse(r)
    }
}

impl Body for EllipticalArc2 {
    const TAG: u8 = ELLIPTICAL_ARC;

    fn write_body(&self, w: &mut Writer) {
        write_ellipse(w, self.ellipse());
        w.f64(self.start_parameter());
        w.f64(self.sweep());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let ellipse = read_ellipse(r)?;
        let at = r.pos;
        let (start, sweep) = (r.f64()?, r.f64()?);
        constructed(at, Self::new(ellipse, start, sweep))
    }
}

impl Body for Polyline2 {
    const TAG: u8 = POLYLINE;

    fn write_body(&self, w: &mut Writer) {
        w.flag(self.is_closed());
        w.count(self.vertices().len());
        for v in self.vertices() {
            w.point2(&v.point);
            w.f64(v.bulge);
            w.f64(v.start_width);
            w.f64(v.end_width);
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let closed = r.flag()?;
        let count = r.count(MIN_VERTEX)?;
        let mut vertices = Vec::with_capacity(count);
        for _ in 0..count {
            vertices.push(PolylineVertex {
                point: r.point2()?,
                bulge: r.f64()?,
                start_width: r.f64()?,
                end_width: r.f64()?,
            });
        }
        constructed(at, Self::new(vertices, closed))
    }
}

impl Value for Curve2 {
    fn write(&self, w: &mut Writer) {
        match self {
            Self::Segment(c) => c.write(w),
            Self::Ray(c) => c.write(w),
            Self::Line(c) => c.write(w),
            Self::Circle(c) => c.write(w),
            Self::Arc(c) => c.write(w),
            Self::Ellipse(c) => c.write(w),
            Self::EllipticalArc(c) => c.write(w),
            Self::Polyline(c) => c.write(w),
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self> {
        Ok(match r.peek()? {
            SEGMENT => Self::Segment(Segment2::read(r)?),
            RAY => Self::Ray(Ray2::read(r)?),
            LINE => Self::Line(Line2::read(r)?),
            CIRCLE => Self::Circle(Circle2::read(r)?),
            ARC => Self::Arc(Arc2::read(r)?),
            ELLIPSE => Self::Ellipse(Ellipse2::read(r)?),
            ELLIPTICAL_ARC => Self::EllipticalArc(EllipticalArc2::read(r)?),
            POLYLINE => Self::Polyline(Polyline2::read(r)?),
            _ => return Err(invalid(r.pos, "unknown curve kind tag")),
        })
    }
}

impl Body for Loop2 {
    const TAG: u8 = LOOP;

    fn write_body(&self, w: &mut Writer) {
        w.count(self.curves().len());
        for curve in self.curves() {
            curve.write(w);
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let count = r.count(MIN_CURVE)?;
        let curves = r.nested_values(count)?;
        constructed(at, Self::from_stored(curves))
    }
}

impl Body for Region {
    const TAG: u8 = REGION;

    fn write_body(&self, w: &mut Writer) {
        w.count(self.parts().len());
        for part in self.parts() {
            part.outer().write(w);
            w.count(part.holes().len());
            for hole in part.holes() {
                hole.write(w);
            }
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let count = r.count(MIN_PART)?;
        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            let outer = r.nested()?;
            let holes = r.count(MIN_LOOP)?;
            parts.push(RegionPart::new(outer, r.nested_values(holes)?));
        }
        constructed(at, Self::new(parts))
    }
}

impl Body for Aabb2 {
    const TAG: u8 = AABB2;

    fn write_body(&self, w: &mut Writer) {
        w.flag(!self.is_empty());
        if !self.is_empty() {
            w.point2(&self.min());
            w.point2(&self.max());
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        if !r.flag()? {
            return Ok(Self::EMPTY);
        }
        let at = r.pos;
        let (min, max) = (r.point2()?, r.point2()?);
        if min.x <= max.x && min.y <= max.y {
            Ok(Self::from_corners(min, max))
        } else {
            Err(invalid(at, "minimum corner above the maximum corner"))
        }
    }
}

impl Body for Aabb3 {
    const TAG: u8 = AABB3;

    fn write_body(&self, w: &mut Writer) {
        w.flag(!self.is_empty());
        if !self.is_empty() {
            w.point3(&self.min());
            w.point3(&self.max());
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        if !r.flag()? {
            return Ok(Self::EMPTY);
        }
        let at = r.pos;
        let (min, max) = (r.point3()?, r.point3()?);
        if min.x <= max.x && min.y <= max.y && min.z <= max.z {
            Ok(Self::from_corners(min, max))
        } else {
            Err(invalid(at, "minimum corner above the maximum corner"))
        }
    }
}

impl Body for ViewFrame {
    const TAG: u8 = VIEW_FRAME;

    fn write_body(&self, w: &mut Writer) {
        w.point3(&self.eye());
        w.point3(&self.target());
        let up = self.up_hint();
        w.point3(&Point3::new(up.x, up.y, up.z));
        match self.projection() {
            Projection::Perspective { vertical_fov } => {
                w.u8(0);
                w.f64(vertical_fov);
            }
            Projection::Orthographic { height } => {
                w.u8(1);
                w.f64(height);
            }
        }
        w.f64(self.near());
        w.f64(self.far());
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let (eye, target, up) = (r.point3()?, r.point3()?, r.vector3()?);
        let kind_at = r.pos;
        let projection = match r.u8()? {
            0 => Projection::Perspective {
                vertical_fov: r.f64()?,
            },
            1 => Projection::Orthographic { height: r.f64()? },
            _ => return Err(invalid(kind_at, "unknown projection kind")),
        };
        let (near, far) = (r.f64()?, r.f64()?);
        constructed(at, view_frame(eye, target, up, projection, near, far))
    }
}

impl Body for TriangleMesh2 {
    const TAG: u8 = TRIANGLE_MESH;

    fn write_body(&self, w: &mut Writer) {
        w.count(self.vertices().len());
        for p in self.vertices() {
            w.point2(p);
        }
        w.count(self.triangles().len());
        for triangle in self.triangles() {
            for index in triangle {
                w.put(&index.to_le_bytes());
            }
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        let count = r.count(MIN_POINT)?;
        let mut vertices = Vec::with_capacity(count);
        for _ in 0..count {
            vertices.push(r.point2()?);
        }
        let count = r.count(MIN_TRIANGLE)?;
        let mut triangles = Vec::with_capacity(count);
        for _ in 0..count {
            triangles.push([r.u32()?, r.u32()?, r.u32()?]);
        }
        constructed(at, Self::new(vertices, triangles))
    }
}

macro_rules! codec {
    ($($ty:ty),*) => {
        $(impl GeomCodec for $ty {})*
    };
}

codec!(
    Segment2,
    Ray2,
    Line2,
    Circle2,
    Arc2,
    Ellipse2,
    EllipticalArc2,
    Polyline2,
    Curve2,
    Loop2,
    Region,
    Aabb2,
    Aabb3,
    ViewFrame,
    TriangleMesh2
);

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use arc_base::error::Value as Detail;
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, extreme_curve};
    use crate::region::tests::region;
    use crate::testing::{extreme_coordinate, extreme_point, point};

    const ALL_TAGS: [u8; 14] = [
        SEGMENT,
        RAY,
        LINE,
        CIRCLE,
        ARC,
        ELLIPSE,
        ELLIPTICAL_ARC,
        POLYLINE,
        LOOP,
        REGION,
        AABB2,
        AABB3,
        VIEW_FRAME,
        TRIANGLE_MESH,
    ];

    fn reason(error: &Error) -> String {
        assert_eq!(error.code(), "geom.decode.invalid", "{error:?}");
        match error.details().get("reason") {
            Some(Detail::Text(text)) => text.clone(),
            other => panic!("no reason: {other:?}"),
        }
    }

    fn header(tag: u8) -> Vec<u8> {
        let mut bytes = b"ARCG\x01\x00".to_vec();
        bytes.push(tag);
        bytes
    }

    /// Decoding either fails with `geom.decode.invalid` or returns a value that encodes to
    /// exactly the input.
    fn check<T: GeomCodec + Debug>(bytes: &[u8]) {
        match T::decode(bytes, &Limits::STRICT) {
            Ok(value) => assert_eq!(value.encode(), bytes, "{value:?}"),
            Err(error) => assert_eq!(error.code(), "geom.decode.invalid", "{error:?}"),
        }
    }

    fn check_every_type(bytes: &[u8]) {
        check::<Segment2>(bytes);
        check::<Ray2>(bytes);
        check::<Line2>(bytes);
        check::<Circle2>(bytes);
        check::<Arc2>(bytes);
        check::<Ellipse2>(bytes);
        check::<EllipticalArc2>(bytes);
        check::<Polyline2>(bytes);
        check::<Curve2>(bytes);
        check::<Loop2>(bytes);
        check::<Region>(bytes);
        check::<Aabb2>(bytes);
        check::<Aabb3>(bytes);
        check::<ViewFrame>(bytes);
        check::<TriangleMesh2>(bytes);
    }

    /// Round trip, canonical length, and mutation, truncation and extension of the encoding.
    fn exercise<T: GeomCodec + Debug + PartialEq>(value: &T, xor: u8, positions: &[usize]) {
        let bytes = value.encode();
        assert_eq!(bytes.len(), value.encoded_len());
        assert_eq!(&T::decode(&bytes, &Limits::STRICT).unwrap(), value);
        for &at in positions {
            let mut mutated = bytes.clone();
            if let Some(byte) = mutated.get_mut(at % bytes.len()) {
                *byte ^= xor;
            }
            check::<T>(&mutated);
            check_every_type(&mutated);
        }
        for &at in positions {
            let truncated = &bytes[..at % bytes.len()];
            reason(&T::decode(truncated, &Limits::STRICT).unwrap_err());
        }
        let mut extended = bytes.clone();
        extended.push(xor);
        let error = T::decode(&extended, &Limits::STRICT).unwrap_err();
        assert_eq!(reason(&error), "trailing bytes after the value");
        let (decoded, rest) = T::decode_prefix(&extended, &Limits::STRICT).unwrap();
        assert_eq!((&decoded, rest), (value, &[xor][..]));
    }

    fn every_position(value: &impl GeomCodec) -> Vec<usize> {
        (0..value.encoded_len()).collect()
    }

    fn neg_zero(x: f64) -> f64 {
        if x == 0.0 { -0.0 } else { x }
    }

    fn neg_point(p: Point2) -> Point2 {
        Point2::new(neg_zero(p.x), neg_zero(p.y))
    }

    fn neg_vector(v: Vector2) -> Vector2 {
        Vector2::new(neg_zero(v.x), neg_zero(v.y))
    }

    /// The same curve with every zero written as −0.0.
    fn negated_zeros(curve: &Curve2) -> Curve2 {
        let ellipse = |e: &Ellipse2| {
            Ellipse2::new(neg_point(e.center()), neg_vector(e.major_axis()), e.ratio()).unwrap()
        };
        match curve {
            Curve2::Segment(c) => {
                Curve2::Segment(Segment2::new(neg_point(c.start()), neg_point(c.end())).unwrap())
            }
            Curve2::Ray(c) => {
                Curve2::Ray(Ray2::new(neg_point(c.origin()), neg_vector(c.direction())).unwrap())
            }
            Curve2::Line(c) => {
                Curve2::Line(Line2::new(neg_point(c.point()), neg_vector(c.direction())).unwrap())
            }
            Curve2::Circle(c) => {
                Curve2::Circle(Circle2::new(neg_point(c.center()), c.radius()).unwrap())
            }
            Curve2::Arc(c) => Curve2::Arc(
                Arc2::new(
                    neg_point(c.center()),
                    c.radius(),
                    neg_zero(c.start_angle()),
                    c.sweep(),
                )
                .unwrap(),
            ),
            Curve2::Ellipse(c) => Curve2::Ellipse(ellipse(c)),
            Curve2::EllipticalArc(c) => Curve2::EllipticalArc(
                EllipticalArc2::new(
                    ellipse(c.ellipse()),
                    neg_zero(c.start_parameter()),
                    c.sweep(),
                )
                .unwrap(),
            ),
            Curve2::Polyline(c) => {
                let vertices = c
                    .vertices()
                    .iter()
                    .map(|v| PolylineVertex {
                        point: neg_point(v.point),
                        bulge: neg_zero(v.bulge),
                        start_width: neg_zero(v.start_width),
                        end_width: neg_zero(v.end_width),
                    })
                    .collect();
                Curve2::Polyline(Polyline2::new(vertices, c.is_closed()).unwrap())
            }
        }
    }

    /// The curve encoded as the type it holds; the bytes must be those of the curve.
    fn inner_encoding(curve: &Curve2) -> Vec<u8> {
        fn round<T: GeomCodec + Debug + PartialEq>(value: &T) -> Vec<u8> {
            let bytes = value.encode();
            assert_eq!(&T::decode(&bytes, &Limits::STRICT).unwrap(), value);
            bytes
        }
        match curve {
            Curve2::Segment(c) => round(c),
            Curve2::Ray(c) => round(c),
            Curve2::Line(c) => round(c),
            Curve2::Circle(c) => round(c),
            Curve2::Arc(c) => round(c),
            Curve2::Ellipse(c) => round(c),
            Curve2::EllipticalArc(c) => round(c),
            Curve2::Polyline(c) => round(c),
        }
    }

    fn any_curve() -> impl Strategy<Value = Curve2> {
        prop_oneof![curve(), extreme_curve()]
    }

    fn aabb2() -> impl Strategy<Value = Aabb2> {
        prop_oneof![
            1 => Just(Aabb2::EMPTY),
            8 => (extreme_point(), extreme_point()).prop_map(|(a, b)| Aabb2::from_corners(a, b)),
        ]
    }

    fn aabb3() -> impl Strategy<Value = Aabb3> {
        let p = || {
            (
                extreme_coordinate(),
                extreme_coordinate(),
                extreme_coordinate(),
            )
                .prop_map(|(x, y, z)| Point3::new(x, y, z))
        };
        prop_oneof![
            1 => Just(Aabb3::EMPTY),
            8 => (p(), p()).prop_map(|(a, b)| Aabb3::from_corners(a, b)),
        ]
    }

    fn view() -> impl Strategy<Value = ViewFrame> {
        let p = || (-50.0..50.0_f64, -50.0..50.0_f64, -50.0..50.0_f64);
        let projection = prop_oneof![
            (0.2..3.0_f64).prop_map(|vertical_fov| Projection::Perspective { vertical_fov }),
            (0.5..50.0_f64).prop_map(|height| Projection::Orthographic { height }),
        ];
        (p(), p(), p(), projection, -1.0..5.0_f64, 1.0..200.0_f64).prop_filter_map(
            "valid frame",
            |(e, t, u, projection, near, span)| {
                view_frame(
                    Point3::new(e.0, e.1, e.2),
                    Point3::new(t.0, t.1, t.2),
                    Vector3::new(u.0, u.1, u.2),
                    projection,
                    near,
                    near + span,
                )
                .ok()
            },
        )
    }

    fn mesh() -> impl Strategy<Value = TriangleMesh2> {
        prop::collection::vec(point(), 1..8).prop_flat_map(|vertices| {
            let n = u32::try_from(vertices.len()).unwrap();
            prop::collection::vec([0..n, 0..n, 0..n], 0..6)
                .prop_map(move |triangles| TriangleMesh2::new(vertices.clone(), triangles).unwrap())
        })
    }

    fn positions() -> impl Strategy<Value = Vec<usize>> {
        prop::collection::vec(any::<usize>(), 24)
    }

    proptest! {
        #[test]
        fn curves_round_trip_canonically(c in any_curve(), xor in 1..=255_u8) {
            let bytes = c.encode();
            prop_assert_eq!(&inner_encoding(&c), &bytes);
            let twin = negated_zeros(&c);
            prop_assert_eq!(&twin, &c);
            prop_assert_eq!(&twin.encode(), &bytes);
            exercise(&c, xor, &every_position(&c));
        }

        #[test]
        fn regions_and_loops_round_trip_canonically(
            r in region(),
            xor in 1..=255_u8,
            at in positions(),
        ) {
            exercise(&r, xor, &at);
            for part in r.parts() {
                exercise(part.outer(), xor, &at);
                for hole in part.holes() {
                    prop_assert_eq!(&Loop2::decode(&hole.encode(), &Limits::STRICT).unwrap(), hole);
                }
            }
        }

        #[test]
        fn boxes_frames_and_meshes_round_trip_canonically(
            a in aabb2(),
            b in aabb3(),
            f in view(),
            m in mesh(),
            xor in 1..=255_u8,
        ) {
            exercise(&a, xor, &every_position(&a));
            exercise(&b, xor, &every_position(&b));
            exercise(&f, xor, &every_position(&f));
            exercise(&m, xor, &every_position(&m));
            let twin = TriangleMesh2::new(
                m.vertices().iter().map(|p| neg_point(*p)).collect(),
                m.triangles().to_vec(),
            ).unwrap();
            prop_assert_eq!(twin.encode(), m.encode());
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
            check_every_type(&bytes);
        }

        #[test]
        fn arbitrary_bodies_never_panic(
            tag in prop::sample::select(ALL_TAGS.to_vec()),
            body in prop::collection::vec(any::<u8>(), 0..400),
        ) {
            let mut bytes = header(tag);
            bytes.extend(body);
            check_every_type(&bytes);
        }
    }

    #[test]
    fn header_kind_and_values_are_checked() {
        let segment = Segment2::new(Point2::new(0.0, 0.0), Point2::new(1.0, 2.0)).unwrap();
        let bytes = segment.encode();
        let decode = |bytes: &[u8]| reason(&Segment2::decode(bytes, &Limits::STRICT).unwrap_err());
        let with = |at: usize, value: &[u8]| {
            let mut copy = bytes.clone();
            copy.splice(at..at + value.len(), value.iter().copied());
            copy
        };
        assert_eq!(decode(&with(0, b"ARCH")), "not a geometry encoding");
        assert_eq!(decode(&with(4, &[2, 0])), "unsupported encoding version");
        assert_eq!(decode(&with(4, &[0, 0])), "unsupported encoding version");
        assert_eq!(decode(&with(6, &[RAY])), "unexpected kind tag");
        for tag in [0x00, 0x18, 0x1F, 0x60, 0x70, 0xFF] {
            assert_eq!(
                reason(&Curve2::decode(&with(6, &[tag]), &Limits::STRICT).unwrap_err()),
                "unknown curve kind tag"
            );
            assert_eq!(decode(&with(6, &[tag])), "unexpected kind tag");
        }
        assert_eq!(
            decode(&with(7, &f64::NAN.to_le_bytes())),
            "number is not finite"
        );
        assert_eq!(
            decode(&with(7, &f64::INFINITY.to_le_bytes())),
            "number is not finite"
        );
        assert_eq!(decode(&with(7, &(-0.0_f64).to_le_bytes())), "negative zero");
        // The end equal to the start: the constructor's code is kept.
        let error = Segment2::decode(&with(23, &bytes[7..23]), &Limits::STRICT).unwrap_err();
        assert_eq!(
            error.details().get("cause"),
            Some(&Detail::Text("geom.segment.zero_length".into()))
        );
        assert_eq!(error.details().get("offset"), Some(&Detail::Int(7)));
        assert_eq!(decode(&[]), "unexpected end of input");
    }

    #[test]
    fn non_canonical_input_is_rejected() {
        // An arc start angle outside [0, 2π) is normalized by the constructor.
        let arc = Arc2::new(Point2::origin(), 1.0, 1.0, 1.0).unwrap();
        let mut bytes = arc.encode();
        bytes.splice(31..39, (1.0 + std::f64::consts::TAU).to_le_bytes());
        assert_eq!(
            reason(&Arc2::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "not the canonical encoding of the value"
        );
        // A bulge on the last vertex of an open polyline is set to 0 by the constructor.
        let open = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::origin(), 0.5),
                PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let mut bytes = open.encode();
        let last_bulge = bytes.len() - 24;
        bytes.splice(last_bulge..last_bulge + 8, 0.5_f64.to_le_bytes());
        assert!(Polyline2::decode(&bytes, &Limits::STRICT).is_err());
        // A clockwise outer loop is turned by the region constructor.
        let square = crate::region::tests::square(0.0, 0.0, 1.0);
        let region = Region::new(vec![RegionPart::new(square.clone(), vec![])]).unwrap();
        let mut bytes = region.encode();
        let outer = square.reversed().encode();
        bytes.splice(15.., outer[6..].iter().copied().chain([0; 8]));
        assert_eq!(
            reason(&Region::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "not the canonical encoding of the value"
        );
        // A box flag other than 0 or 1, and crossed corners.
        let mut bytes = Aabb2::EMPTY.encode();
        bytes[7] = 2;
        assert_eq!(
            reason(&Aabb2::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "flag is neither 0 nor 1"
        );
        let boxed = Aabb2::from_corners(Point2::origin(), Point2::new(1.0, 1.0));
        let mut bytes = boxed.encode();
        bytes.splice(
            8..24,
            Point2::new(2.0, 2.0)
                .coords
                .iter()
                .flat_map(|c| c.to_le_bytes()),
        );
        assert_eq!(
            reason(&Aabb2::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "minimum corner above the maximum corner"
        );
    }

    #[test]
    fn limits_are_enforced_before_allocating() {
        let mut bytes = header(POLYLINE);
        bytes.push(0);
        bytes.extend(u64::MAX.to_le_bytes());
        assert_eq!(
            reason(&Polyline2::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "count exceeds the array length limit"
        );
        let unlimited = Limits {
            array_length: u64::MAX,
            ..Limits::STRICT
        };
        assert_eq!(
            reason(&Polyline2::decode(&bytes, &unlimited).unwrap_err()),
            "count exceeds the remaining input"
        );
        let mut bytes = header(TRIANGLE_MESH);
        bytes.extend(1_000_u64.to_le_bytes());
        bytes.extend([0; 64]);
        assert_eq!(
            reason(&TriangleMesh2::decode(&bytes, &Limits::STRICT).unwrap_err()),
            "count exceeds the remaining input"
        );

        let polyline = crate::region::tests::square(0.0, 0.0, 1.0);
        let bytes = polyline.encode();
        let tight = Limits {
            array_length: 3,
            ..Limits::STRICT
        };
        assert_eq!(
            reason(&Loop2::decode(&bytes, &tight).unwrap_err()),
            "count exceeds the array length limit"
        );
        let small = Limits {
            input_bytes: 10,
            ..Limits::STRICT
        };
        assert_eq!(
            reason(&Loop2::decode(&bytes, &small).unwrap_err()),
            "input exceeds the input byte limit"
        );

        let region = Region::new(vec![RegionPart::new(polyline, vec![])]).unwrap();
        let bytes = region.encode();
        for (depth, ok) in [(0, false), (2, false), (3, true)] {
            let limits = Limits {
                nesting_depth: depth,
                ..Limits::STRICT
            };
            match Region::decode(&bytes, &limits) {
                Ok(decoded) => assert!(ok && decoded == region),
                Err(error) => {
                    assert!(!ok);
                    assert_eq!(reason(&error), "value exceeds the nesting depth limit");
                }
            }
        }
    }

    #[test]
    fn values_follow_one_another_in_a_stream() {
        let a = Curve2::Circle(Circle2::new(Point2::origin(), 2.0).unwrap());
        let b = Aabb2::from_corners(Point2::origin(), Point2::new(1.0, 1.0));
        let mut stream = a.encode();
        stream.extend(b.encode());
        let (first, rest) = Curve2::decode_prefix(&stream, &Limits::STRICT).unwrap();
        let (second, rest) = Aabb2::decode_prefix(rest, &Limits::STRICT).unwrap();
        assert_eq!((first, second, rest.len()), (a, b, 0));
    }

    #[test]
    fn a_polyline_arc_encodes_its_center_radius_form() {
        let polyline = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::origin(), 0.5),
                PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let Some(crate::PolylineSegment::Arc(arc)) = polyline.segment(0) else {
            panic!("not an arc");
        };
        let plain = Arc2::new(arc.center(), arc.radius(), arc.start_angle(), arc.sweep()).unwrap();
        assert_eq!(arc.encode(), plain.encode());
        assert_eq!(Arc2::decode(&arc.encode(), &Limits::STRICT).unwrap(), plain);
    }
}
