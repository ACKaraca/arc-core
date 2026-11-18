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
//!   −0.0 is written as +0.0, and every other number is written bit for bit. Decoding an
//!   encoding gives the value back. An arc of a polyline segment
//!   ([`crate::Polyline2::segment`]) keeps its chord: it is written in its chord form (start,
//!   end, bulge) under its own tag and rebuilt from it, so its end points stay the vertices
//!   exactly and a loop that holds it reads back. A chord arc and the plain arc with the same
//!   center, radius and angles are different values with different bytes.
//! - **Unique.** The decoder accepts only the canonical form: it builds every value with the
//!   type's validating constructor, encodes the result again and rejects the input unless the
//!   bytes are identical, so input that a constructor would normalize (an arc start angle
//!   outside [0, 2π), a bulge on the last vertex of an open polyline, a region loop with the
//!   wrong orientation) is rejected.
//! - **Bounded.** Decoding takes [`Limits`]: the bytes a value occupies are checked against
//!   `input_bytes`, every count against `array_length` and against the input that remains
//!   before anything is reserved, and the nesting of values (at most three levels: region,
//!   loop, curve, counted from 1 in every call) against `nesting_depth`. Nothing recurses beyond
//!   that depth. Memory for an array is reserved fallibly: a reservation that fails is an error,
//!   never a panic or an abort.
//!
//! The byte contract is structural; the geometric validation of loops and regions that the
//! decoder applies on top of it is described in `docs/architecture/geometry-encoding.md`. A
//! bounding box with an infinite corner (`from_corners` accepts one) is not storable: its
//! encoding is rejected by the decoder, as its serialized form is by deserialization.
//!
//! Every failure is `geom.decode.invalid` with the details `offset` (the byte position) and
//! `reason`; a value that its constructor rejects also carries the constructor's code as
//! `cause` and the constructor's error as the source.

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::nurbs::Nurbs2;
use crate::{
    Aabb2, Aabb3, Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, Line2, Loop2, Polyline2,
    PolylineSegment, PolylineVertex, Projection, Ray2, Region, RegionPart, Segment2, TriangleMesh2,
    ViewFrame,
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
    /// Version 1, in which this crate writes every value that holds no NURBS curve.
    pub const CURRENT: Self = Self {
        name: "arc.geom",
        version: 1,
    };

    /// Version 2: version 1 and the NURBS curve kind. This crate writes a value in it exactly
    /// when the value holds a NURBS curve, and reads both versions.
    pub const VERSION_2: Self = Self {
        name: "arc.geom",
        version: 2,
    };

    /// The four bytes every encoded value starts with.
    pub const MAGIC: [u8; 4] = *b"ARCG";
}

// Kind tags. 0x19 is the NURBS curve of version 2; 0x1A-0x1F are reserved for further 2D curve
// kinds (spirals, composites),
// 0x60-0x6F for 3D curves, 0x70-0x7F for surfaces; every unassigned tag is rejected.
const SEGMENT: u8 = 0x10;
const RAY: u8 = 0x11;
const LINE: u8 = 0x12;
const CIRCLE: u8 = 0x13;
const ARC: u8 = 0x14;
const ELLIPSE: u8 = 0x15;
const ELLIPTICAL_ARC: u8 = 0x16;
const POLYLINE: u8 = 0x17;
const CHORD_ARC: u8 = 0x18;
const NURBS: u8 = 0x19;
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
///
/// An [`Arc2`] has two kinds: the plain arc, and the chord arc of a polyline segment;
/// [`Arc2::decode`] reads both.
pub trait GeomCodec: sealed::Value {
    /// The canonical encoding: header and value.
    #[must_use]
    fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new(self.encoded_len());
        writer.header(self.version());
        self.write(&mut writer);
        writer.bytes
    }

    /// The length of [`Self::encode`] in bytes, computed without allocating.
    #[must_use]
    fn encoded_len(&self) -> usize {
        let mut writer = Writer::counter();
        writer.header(self.version());
        self.write(&mut writer);
        writer.len
    }

    /// The encoding [`Self::encode`] writes: [`GeomEncoding::VERSION_2`] for a value that holds a
    /// NURBS curve, [`GeomEncoding::CURRENT`] (version 1) otherwise.
    #[must_use]
    fn encoding(&self) -> GeomEncoding {
        if self.version() == GeomEncoding::VERSION_2.version {
            GeomEncoding::VERSION_2
        } else {
            GeomEncoding::CURRENT
        }
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
    /// Only the bytes of the value count toward `input_bytes`, not the bytes after it.
    fn decode_prefix<'a>(bytes: &'a [u8], limits: &Limits) -> Result<(Self, &'a [u8])> {
        // The reader sees at most `input_bytes` bytes, so a value that needs more ends early.
        let window = usize::try_from(limits.input_bytes)
            .ok()
            .filter(|limit| *limit < bytes.len())
            .and_then(|limit| bytes.get(..limit));
        let mut reader = Reader {
            bytes: window.unwrap_or(bytes),
            pos: 0,
            limits,
            depth: 0,
            limited: window.is_some(),
            version: 0,
        };
        if reader.take::<4>()? != GeomEncoding::MAGIC {
            return Err(invalid(0, "not a geometry encoding"));
        }
        let version = u16::from_le_bytes(reader.take()?);
        if version != GeomEncoding::CURRENT.version && version != GeomEncoding::VERSION_2.version {
            return Err(invalid(4, "unsupported encoding version"));
        }
        reader.version = version;
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
        /// Whether `bytes` was cut at the input byte limit.
        pub(super) limited: bool,
        /// The version in the header.
        pub(super) version: u16,
    }

    /// A value with a kind tag and a body; not implementable outside this crate.
    pub trait Value: Sized {
        /// Writes the tag and the body.
        fn write(&self, writer: &mut Writer);
        /// Reads the tag and the body.
        fn read(reader: &mut Reader<'_>) -> Result<Self>;
        /// The lowest encoding version that can hold the value.
        fn version(&self) -> u16 {
            1
        }
    }

    /// A value of one kind tag.
    pub trait Body: Sized {
        const TAG: u8;
        fn write_body(&self, writer: &mut Writer);
        fn read_body(reader: &mut Reader<'_>) -> Result<Self>;
        fn body_version(&self) -> u16 {
            1
        }
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

        fn version(&self) -> u16 {
            self.body_version()
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

    fn header(&mut self, version: u16) {
        self.put(&GeomEncoding::MAGIC);
        self.put(&version.to_le_bytes());
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
    /// The error for a value that needs more bytes than the reader has.
    fn ended(&self, at: usize, reason: &'static str) -> Error {
        if self.limited {
            invalid(self.bytes.len(), "input exceeds the input byte limit")
        } else {
            invalid(at, reason)
        }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self
            .pos
            .checked_add(N)
            .and_then(|end| self.bytes.get(self.pos..end))
            .and_then(|slice| <[u8; N]>::try_from(slice).ok())
            .ok_or_else(|| self.ended(self.pos, "unexpected end of input"))?;
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
            .ok_or_else(|| self.ended(self.pos, "unexpected end of input"))
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

    /// A count of elements whose encoding takes at least `min_size` bytes each, with an empty
    /// vector that has room for them. The count is checked against the array length limit and
    /// the remaining input first, so the reservation is at most `size_of::<T>() / min_size`
    /// times the remaining input, and a reservation that fails is an error.
    fn counted<T>(&mut self, min_size: u64) -> Result<(usize, Vec<T>)> {
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
            return Err(self.ended(at, "count exceeds the remaining input"));
        }
        let count =
            usize::try_from(count).map_err(|_| invalid(at, "count exceeds the address space"))?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| invalid(at, "count exceeds the available memory"))?;
        Ok((count, values))
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

    /// Reads a count and that many values one nesting level deeper.
    fn nested_values<T: Value>(&mut self, min_size: u64) -> Result<Vec<T>> {
        let (count, mut values) = self.counted(min_size)?;
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

/// The arc of the polyline segment from `start` to `end` with `bulge`, exactly as
/// [`Polyline2::segment`] builds it.
pub(crate) fn chord_arc(start: Point2, end: Point2, bulge: f64) -> Result<Option<Arc2>> {
    let vertices = vec![
        PolylineVertex::new(start, bulge),
        PolylineVertex::new(end, 0.0),
    ];
    Ok(match Polyline2::new(vertices, false)?.segment(0) {
        Some(PolylineSegment::Arc(arc)) => Some(arc),
        _ => None,
    })
}

// An arc has two kinds: the plain arc, and the chord arc of a polyline segment.
impl Value for Arc2 {
    fn write(&self, w: &mut Writer) {
        if let Some(chord) = self.chord() {
            w.u8(CHORD_ARC);
            w.point2(&chord.start());
            w.point2(&chord.end());
            w.f64(chord.bulge());
        } else {
            w.u8(ARC);
            w.point2(&self.center());
            w.f64(self.radius());
            w.f64(self.start_angle());
            w.f64(self.sweep());
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self> {
        let tag_at = r.pos;
        let tag = r.u8()?;
        let at = r.pos;
        match tag {
            ARC => {
                let (center, radius, start, sweep) = (r.point2()?, r.f64()?, r.f64()?, r.f64()?);
                constructed(at, Self::new(center, radius, start, sweep))
            }
            CHORD_ARC => {
                let (start, end, bulge) = (r.point2()?, r.point2()?, r.f64()?);
                constructed(at, chord_arc(start, end, bulge))?
                    .ok_or_else(|| invalid(at, "chord arc with a zero bulge"))
            }
            _ => Err(invalid(tag_at, "unexpected kind tag")),
        }
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
        let (count, mut vertices) = r.counted(MIN_VERTEX)?;
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

/// The smallest encoding of a control point: point and weight.
const MIN_CONTROL_POINT: u64 = 24;

impl Body for Nurbs2 {
    const TAG: u8 = NURBS;

    fn write_body(&self, w: &mut Writer) {
        w.count(self.degree());
        w.count(self.control_points().len());
        for (p, weight) in self.control_points().iter().zip(self.weights()) {
            w.point2(p);
            w.f64(*weight);
        }
        for knot in self.knots() {
            w.f64(*knot);
        }
    }

    fn read_body(r: &mut Reader<'_>) -> Result<Self> {
        let at = r.pos;
        if r.version < GeomEncoding::VERSION_2.version {
            return Err(invalid(
                at.saturating_sub(1),
                "kind tag not in this encoding version",
            ));
        }
        let degree = u64::from_le_bytes(r.take()?);
        let degree = usize::try_from(degree)
            .ok()
            .filter(|d| (1..=Nurbs2::MAX_DEGREE).contains(d))
            .ok_or_else(|| invalid(at, "degree out of range"))?;
        let (count, mut points) = r.counted::<Point2>(MIN_CONTROL_POINT)?;
        let mut weights = Vec::new();
        weights
            .try_reserve_exact(count)
            .map_err(|_| invalid(at, "count exceeds the available memory"))?;
        for _ in 0..count {
            points.push(r.point2()?);
            weights.push(r.f64()?);
        }
        let knot_count = count + degree + 1;
        let knots_at = r.pos;
        let remaining = r.bytes.len() - r.pos;
        if knot_count
            .checked_mul(8)
            .is_none_or(|needed| needed > remaining)
        {
            return Err(r.ended(knots_at, "count exceeds the remaining input"));
        }
        let mut knots = Vec::new();
        knots
            .try_reserve_exact(knot_count)
            .map_err(|_| invalid(knots_at, "count exceeds the available memory"))?;
        for _ in 0..knot_count {
            knots.push(r.f64()?);
        }
        constructed(
            at,
            Nurbs2::with_limits(degree, knots, points, weights, r.limits),
        )
    }

    fn body_version(&self) -> u16 {
        GeomEncoding::VERSION_2.version
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
            Self::Nurbs(c) => c.write(w),
        }
    }

    fn version(&self) -> u16 {
        match self {
            Self::Nurbs(c) => c.version(),
            _ => 1,
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self> {
        Ok(match r.peek()? {
            SEGMENT => Self::Segment(Segment2::read(r)?),
            RAY => Self::Ray(Ray2::read(r)?),
            LINE => Self::Line(Line2::read(r)?),
            CIRCLE => Self::Circle(Circle2::read(r)?),
            ARC | CHORD_ARC => Self::Arc(Arc2::read(r)?),
            ELLIPSE => Self::Ellipse(Ellipse2::read(r)?),
            ELLIPTICAL_ARC => Self::EllipticalArc(EllipticalArc2::read(r)?),
            POLYLINE => Self::Polyline(Polyline2::read(r)?),
            NURBS => Self::Nurbs(Nurbs2::read(r)?),
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
        let curves = r.nested_values(MIN_CURVE)?;
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
        let (count, mut parts) = r.counted(MIN_PART)?;
        for _ in 0..count {
            let outer = r.nested()?;
            parts.push(RegionPart::new(outer, r.nested_values(MIN_LOOP)?));
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
        let (count, mut vertices) = r.counted(MIN_POINT)?;
        for _ in 0..count {
            vertices.push(r.point2()?);
        }
        let (count, mut triangles) = r.counted(MIN_TRIANGLE)?;
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
    Nurbs2,
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

    use arc_base::Tolerance;
    use arc_base::error::Value as Detail;
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, extreme_curve};
    use crate::region::tests::region;
    use crate::testing::{extreme_coordinate, extreme_point, point};

    const ALL_TAGS: [u8; 15] = [
        SEGMENT,
        RAY,
        LINE,
        CIRCLE,
        ARC,
        ELLIPSE,
        ELLIPTICAL_ARC,
        POLYLINE,
        CHORD_ARC,
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
            Curve2::Nurbs(c) => Curve2::Nurbs(
                Nurbs2::new(
                    c.degree(),
                    c.knots().iter().map(|k| neg_zero(*k)).collect(),
                    c.control_points().iter().map(|p| neg_point(*p)).collect(),
                    c.weights().to_vec(),
                )
                .unwrap(),
            ),
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
            Curve2::Nurbs(c) => round(c),
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
            prop::collection::vec([0..n, 0..n, 0..n], 0..6).prop_map(move |triangles| {
                // Only strictly counterclockwise triangles are valid: flip or drop the rest.
                let at = |i: u32| vertices[i as usize];
                let triangles = triangles
                    .into_iter()
                    .filter_map(
                        |[a, b, c]| match crate::orient2d(&at(a), &at(b), &at(c)).ok()? {
                            crate::Sign::Positive => Some([a, b, c]),
                            crate::Sign::Negative => Some([a, c, b]),
                            crate::Sign::Zero => None,
                        },
                    )
                    .collect();
                TriangleMesh2::new(vertices.clone(), triangles).unwrap()
            })
        })
    }

    /// The arc of a polyline segment, with bulges from nearly straight to nearly a full turn.
    fn chord_arc_value() -> impl Strategy<Value = Arc2> {
        (
            point(),
            point(),
            crate::testing::log_uniform(-12.0, 3.0),
            any::<bool>(),
        )
            .prop_filter_map("a representable bulged segment", |(a, b, bulge, flip)| {
                chord_arc(a, b, if flip { -bulge } else { bulge })
                    .ok()
                    .flatten()
            })
    }

    /// A clockwise loop of three arcs of a radius up to 1e12 around the origin, joined within a
    /// tolerance in proportion to the radius.
    fn large_radius_loop() -> impl Strategy<Value = Loop2> {
        (crate::testing::log_uniform(3.0, 12.0), 0.0..6.0_f64).prop_map(|(r, a)| {
            let arc = |start: f64, sweep: f64| {
                Curve2::Arc(Arc2::new(Point2::origin(), r, start, sweep).unwrap())
            };
            let tolerance = Tolerance {
                linear: 1e-8 * r,
                ..Tolerance::DEFAULT
            };
            let curves = vec![
                arc(a, -2.0),
                arc(a - 2.0, -2.0),
                arc(a - 4.0, 4.0 - std::f64::consts::TAU),
            ];
            Loop2::new(curves, &tolerance).unwrap()
        })
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

    }

    /// Every byte position of a region is mutated, so the default case count is small;
    /// `PROPTEST_CASES` overrides it.
    fn region_cases() -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(16)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(region_cases()))]

        #[test]
        fn regions_and_loops_round_trip_canonically(r in region(), xor in 1..=255_u8) {
            exercise(&r, xor, &every_position(&r));
            for part in r.parts() {
                exercise(part.outer(), xor, &every_position(part.outer()));
                for hole in part.holes() {
                    prop_assert_eq!(&Loop2::decode(&hole.encode(), &Limits::STRICT).unwrap(), hole);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn reversed_chord_arcs_round_trip(arc in chord_arc_value()) {
            let reversed = arc.reversed();
            prop_assert_eq!(Arc2::decode(&reversed.encode(), &Limits::STRICT).unwrap(), reversed);
        }

        #[test]
        fn chord_arcs_round_trip_with_their_chord(arc in chord_arc_value(), xor in 1..=255_u8) {
            prop_assert_eq!(arc.encode()[6], CHORD_ARC);
            exercise(&arc, xor, &every_position(&arc));
            let curve = Curve2::Arc(arc);
            prop_assert_eq!(curve.encode(), arc.encode());
            exercise(&curve, xor, &every_position(&curve));
            // The plain arc with the same center, radius and angles is another value.
            let plain =
                Arc2::new(arc.center(), arc.radius(), arc.start_angle(), arc.sweep()).unwrap();
            prop_assert_ne!(plain, arc);
            prop_assert_eq!(plain.encode()[6], ARC);
            // A loop of the arc and its chord reads back, in both directions, however far the
            // center-radius form is from the vertices.
            let (start, end) = (arc.point_at(0.0), arc.point_at(arc.sweep().abs()));
            let back = Curve2::Segment(Segment2::new(end, start).unwrap());
            if let Ok(lp) = Loop2::new(vec![curve, back], &Tolerance::DEFAULT) {
                exercise(&lp, xor, &every_position(&lp));
                let reversed = lp.reversed();
                exercise(&reversed, xor, &every_position(&reversed));
                let region = Region::new(vec![RegionPart::new(lp, vec![])]).unwrap();
                exercise(&region, xor, &every_position(&region));
            }
        }

        #[test]
        fn reoriented_large_radius_loops_round_trip(lp in large_radius_loop(), xor in 1..=255_u8) {
            exercise(&lp, xor, &every_position(&lp));
            let reversed = lp.reversed();
            exercise(&reversed, xor, &every_position(&reversed));
            // The region constructor turns the clockwise loop.
            let region = Region::new(vec![RegionPart::new(lp, vec![])]).unwrap();
            prop_assert!(region.parts()[0].outer().is_counterclockwise());
            exercise(&region, xor, &every_position(&region));
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
        assert_eq!(decode(&with(4, &[3, 0])), "unsupported encoding version");
        assert_eq!(decode(&with(4, &[0, 0])), "unsupported encoding version");
        // A value without a NURBS curve is written in version 1 only.
        assert_eq!(
            decode(&with(4, &[2, 0])),
            "not the canonical encoding of the value"
        );
        assert_eq!(decode(&with(6, &[RAY])), "unexpected kind tag");
        // The NURBS tag belongs to version 2.
        assert_eq!(
            reason(&Curve2::decode(&with(6, &[NURBS]), &Limits::STRICT).unwrap_err()),
            "kind tag not in this encoding version"
        );
        for tag in [0x00, 0x1A, 0x1F, 0x60, 0x70, 0xFF] {
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
    fn a_polyline_arc_keeps_its_chord() {
        let polyline = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::origin(), 1e-12),
                PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let Some(PolylineSegment::Arc(arc)) = polyline.segment(0) else {
            panic!("not an arc");
        };
        let bytes = arc.encode();
        assert_eq!((bytes[6], bytes.len()), (CHORD_ARC, 47));
        assert_eq!(Arc2::decode(&bytes, &Limits::STRICT).unwrap(), arc);
        // The center of this arc is 2.5e11 away: a loop reads back only through the chord.
        let back = Segment2::new(Point2::new(1.0, 0.0), Point2::origin()).unwrap();
        let lp = Loop2::new(
            vec![Curve2::Arc(arc), Curve2::Segment(back)],
            &Tolerance::DEFAULT,
        )
        .unwrap();
        assert_eq!(Loop2::decode(&lp.encode(), &Limits::STRICT).unwrap(), lp);
        let reversed = lp.reversed();
        assert_eq!(
            Loop2::decode(&reversed.encode(), &Limits::STRICT).unwrap(),
            reversed
        );
        // A chord arc without a bulge is not an arc, and a chord of no length is invalid.
        let mut straight = bytes.clone();
        straight.splice(39..47, 0.0_f64.to_le_bytes());
        assert_eq!(
            reason(&Arc2::decode(&straight, &Limits::STRICT).unwrap_err()),
            "chord arc with a zero bulge"
        );
        let mut empty = bytes.clone();
        empty.splice(23..39, [0; 16]);
        let error = Curve2::decode(&empty, &Limits::STRICT).unwrap_err();
        assert_eq!(
            error.details().get("cause"),
            Some(&Detail::Text("geom.polyline.zero_length_segment".into()))
        );
    }

    #[test]
    fn a_reoriented_loop_of_large_arcs_reads_back() {
        let r = 1e12;
        let arc = |start: f64, sweep: f64| {
            Curve2::Arc(Arc2::new(Point2::origin(), r, start, sweep).unwrap())
        };
        let tolerance = Tolerance {
            linear: 1e4,
            ..Tolerance::DEFAULT
        };
        let curves = vec![
            arc(0.3, -2.0),
            arc(0.3 - 2.0, -2.0),
            arc(0.3 - 4.0, 4.0 - std::f64::consts::TAU),
        ];
        let lp = Loop2::new(curves, &tolerance).unwrap();
        let reversed = lp.reversed();
        assert_eq!(
            Loop2::decode(&reversed.encode(), &Limits::STRICT).unwrap(),
            reversed
        );
        let region = Region::new(vec![RegionPart::new(lp, vec![])]).unwrap();
        assert_eq!(
            Region::decode(&region.encode(), &Limits::STRICT).unwrap(),
            region
        );
    }

    #[test]
    fn only_the_bytes_of_the_value_count_toward_the_input_limit() {
        let circle = Circle2::new(Point2::origin(), 2.0).unwrap();
        let mut stream = circle.encode();
        let len = stream.len();
        stream.extend(circle.encode());
        let exact = Limits {
            input_bytes: len as u64,
            ..Limits::STRICT
        };
        let (first, rest) = Circle2::decode_prefix(&stream, &exact).unwrap();
        assert_eq!((first, rest.len()), (circle, len));
        let short = Limits {
            input_bytes: len as u64 - 1,
            ..Limits::STRICT
        };
        for bytes in [&stream[..], &stream[..len]] {
            let error = Circle2::decode_prefix(bytes, &short).unwrap_err();
            assert_eq!(reason(&error), "input exceeds the input byte limit");
        }
        // A count that the limited input cannot hold is the limit, not a truncation.
        let square = crate::region::tests::square(0.0, 0.0, 1.0).encode();
        let cut = Limits {
            input_bytes: 20,
            ..Limits::STRICT
        };
        assert_eq!(
            reason(&Loop2::decode(&square, &cut).unwrap_err()),
            "input exceeds the input byte limit"
        );
        assert_eq!(
            reason(&Loop2::decode(&square[..20], &cut).unwrap_err()),
            "count exceeds the remaining input"
        );
    }

    #[test]
    fn a_box_with_an_infinite_corner_is_not_storable() {
        let boxed = Aabb2::from_corners(Point2::new(f64::NEG_INFINITY, 0.0), Point2::new(1.0, 1.0));
        assert_eq!(
            reason(&Aabb2::decode(&boxed.encode(), &Limits::STRICT).unwrap_err()),
            "number is not finite"
        );
    }
}
