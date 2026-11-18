# Geometry encoding

The byte form in which `arc-geom` stores geometry values (CORE-GEOM-032): encoding `arc.geom`,
versions 1 and 2. Version 2 is version 1 with the NURBS curve kind added; a value is written in
version 2 exactly when it holds a NURBS curve, and in version 1 otherwise. Package descriptors name this encoding and its version for every geometry field
([ADR-0005](../adr/0005-payload-encoding-with-embedded-descriptors.md)); the decision for a
hand-written layout is recorded in [ADR-0013](../adr/0013-geometry-encoding.md). The API is
`arc_geom::encoding` ([arc-geom](../crates/arc-geom.md#geometry-encoding)).

## Conventions

| Field type | Bytes | Form |
|---|---|---|
| `f64` | 8 | IEEE 754 binary64, little-endian; finite, never −0.0 |
| `u64` | 8 | Unsigned integer, little-endian; used for counts |
| `u32` | 4 | Unsigned integer, little-endian; used for triangle indices |
| `u16` | 2 | Unsigned integer, little-endian; used for the version |
| `flag` | 1 | 0 (false) or 1 (true); any other value is invalid |
| `tag` | 1 | Kind tag, see below |
| `point2`, `vector2` | 16 | `x`, `y` as `f64` |
| `point3`, `vector3` | 24 | `x`, `y`, `z` as `f64` |

Integers have a fixed width, so every value has exactly one integer representation. Counts are
64-bit so that no array a value can hold is truncated.

## Header and values

An encoded value is a header followed by one value. Every top-level value has its own header,
also when values are stored one after another.

| Offset | Field | Type | Content |
|---|---|---|---|
| 0 | magic | 4 bytes | `41 52 43 47` (ASCII `ARCG`) |
| 4 | version | `u16` | `1`, or `2` for a value that holds a NURBS curve |
| 6 | value | | A kind tag and the fields of that kind |

A value is its kind tag followed by its fields in the order of the tables below. A value inside
another value (a curve of a loop, a loop of a region) is a complete value with its own tag and
without a header. A curve stored as `Curve2` has exactly the bytes of the curve kind it holds.

| Tag | Kind | Tag | Kind |
|---|---|---|---|
| `0x10` | Segment | `0x20` | Loop |
| `0x11` | Ray | `0x21` | Region |
| `0x12` | Line | `0x30` | 2D bounding box |
| `0x13` | Circle | `0x31` | 3D bounding box |
| `0x14` | Arc | `0x40` | View frame |
| `0x15` | Ellipse | `0x50` | 2D triangle mesh |
| `0x16` | Elliptical arc | | |
| `0x17` | Polyline | | |
| `0x18` | Chord arc | | |
| `0x19` | NURBS curve (version 2) | | |

The curve kinds are `0x10`–`0x18`, and `0x19` in version 2. Reserved: `0x1A`–`0x1F` for further 2D curve kinds (transition
spirals, composite curves), `0x60`–`0x6F` for 3D curves, `0x70`–`0x7F` for surfaces. Every tag not assigned by the
version being read, including `0x00` and the reserved ranges, is rejected.

## Kinds

Every kind's fields follow its tag, in this order.

| Kind | Fields |
|---|---|
| Segment | `start: point2`, `end: point2` |
| Ray | `origin: point2`, `direction: vector2` (as given, not normalized) |
| Line | `point: point2`, `direction: vector2` (as given, not normalized) |
| Circle | `center: point2`, `radius: f64` |
| Arc | `center: point2`, `radius: f64`, `start_angle: f64` in [0, 2π), `sweep: f64` (signed) |
| Ellipse | `center: point2`, `major_axis: vector2`, `ratio: f64` in (0, 1] |
| Elliptical arc | the ellipse's `center`, `major_axis`, `ratio`, then `start_parameter: f64` in [0, 2π), `sweep: f64` |
| Chord arc | `start: point2`, `end: point2`, `bulge: f64` (not 0): the arc of a polyline segment between two vertices |
| Polyline | `closed: flag`, `count: u64`, then `count` vertices of `point: point2`, `bulge: f64`, `start_width: f64`, `end_width: f64` (40 bytes each) |
| NURBS curve | `degree: u64` in 1..=24, `count: u64` control points, then `count` × (`point: point2`, `weight: f64`) (24 bytes each), then `count + degree + 1` knots as `f64` |
| Loop | `count: u64`, then `count` curve values |
| Region | `parts: u64`, then per part: the outer loop value, `holes: u64`, then `holes` loop values |
| 2D bounding box | `nonempty: flag`; when 1, `min: point2`, `max: point2` |
| 3D bounding box | `nonempty: flag`; when 1, `min: point3`, `max: point3` |
| View frame | `eye: point3`, `target: point3`, `up: vector3` (the up hint as given), `projection: u8` (0 perspective, 1 orthographic), `value: f64` (vertical field of view in radians, or view height), `near: f64`, `far: f64` |
| 2D triangle mesh | `vertices: u64`, then `vertices` × `point2`; `triangles: u64`, then `triangles` × three `u32` vertex indices |

An arc has two kinds, and they are different values. A plain arc is given by its center, radius
and angles and is written as an Arc. The arc of a bulged polyline segment, taken out of its
polyline as a curve of its own, is given by the two vertices and the bulge: it is written as a
Chord arc, its end points are the vertices exactly, and its center, radius and angles are derived
when it is read (center and radius from the chord and the bulge as the
[curve parameterization](../crates/arc-geom.md#curve-parameterization) defines them, sweep
4·atan(bulge)). A writer uses the chord arc exactly when the value was built from a polyline
segment, and the plain arc otherwise; it never converts one into the other, because the
center-radius form of a nearly straight segment does not reproduce the vertices. Both kinds are
accepted wherever a curve or an arc is read.

A full circle or ellipse that bounds a hole of a region runs clockwise, which the Circle and
Ellipse kinds cannot express: it is written as an Arc of sweep −2π, or an Elliptical arc of sweep
−2π (the reverse of a circle or ellipse starts at angle or parameter 0). Likewise an outer loop is never a clockwise full arc.

Derived data (the basis of a view frame, bounding boxes of curves, the center of a chord arc) is
never stored. A bounding box with an infinite corner is not storable: numbers are finite, so the
decoder rejects its bytes.

## Canonical form

The contract of the bytes is structural. A byte string is a well-formed encoding when:

- the header, tags, flags and counts are as the tables above give them, with no bytes missing
  and none left over inside a value;
- every number is finite and no number is −0.0 (the sign of zero is not part of a value: −0.0 is
  written as +0.0), and every other number is written bit for bit, so decoding returns exactly
  the stored numbers;
- every value satisfies the constraints of its kind that depend on its own numbers only: distinct
  segment ends, a nonzero direction, a positive radius, a sweep that is not 0 and at most 2π in
  magnitude, a ratio in (0, 1], at least two polyline vertices with no two consecutive ones equal
  and no negative width, a nonzero bulge of a chord arc, minimum corner not above the maximum,
  triangle indices inside the vertex list; for a NURBS curve a degree below the control point
  count, positive weights, a non-decreasing clamped knot vector (first and last values exactly
  `degree + 1` times, interior values at most `degree` times) and control points not all equal;
- every value is in its normalized form: an arc start angle or elliptical arc start parameter in
  [0, 2π), bulge and widths 0 on the last vertex of an open polyline, and the orientation of
  region loops as stored (outer loops counterclockwise, holes clockwise).

Equal values give equal bytes, and the decoder accepts only this one form: it builds every value
with its type's validating constructor, encodes the value again and rejects the input unless the
bytes are identical. Input that a constructor would normalize is therefore rejected rather than
silently changed.

### Geometric validation by the reader

On top of the structural contract, the reader of this crate validates loops and regions
geometrically, with the default tolerance (`arc_base::Tolerance::DEFAULT`, linear 1e-6):

- consecutive curves of a loop, and the last and the first, must meet within the linear
  tolerance; remaining gaps are kept, not closed with segments;
- the signed area of a loop must be finite and not zero, and its sign decides the orientation that
  the region rule above checks;
- the bounding box of a hole must lie inside that of its outer loop;
- a bulged polyline segment or chord arc must have a center and radius representable in `f64`,
  and a view frame must have finite matrices.

These checks evaluate end points and areas in floating point, through sines and cosines that are
not correctly rounded on every platform. Their outcome is exact and portable away from the
boundary and implementation-defined at it: a loop whose gap is within a few units in the last
place of the tolerance, or whose area is within rounding of zero, may be accepted by one reader
and rejected by another. A writer stays clear of the boundary when it:

- makes consecutive curves share their end point exactly where the kinds allow it (segments,
  polyline vertices, chord arcs), and otherwise keeps every gap below a tenth of the linear
  tolerance, counting the rounding of an arc end point, which is about the radius times 1e-15;
- inserts a segment across any larger gap instead of relying on the tolerance;
- writes no loop whose area is smaller than the square of its size times 1e-12, and orients
  loops by an area computed in the same order of the stored curves.

## Limits and errors
## Limits and errors

`decode` and `decode_prefix` take an `arc_base::Limits` profile:

- The bytes of the value must not exceed `input_bytes`. Bytes after the value that
  `decode_prefix` returns do not count.
- Every count must not exceed `array_length`, and the count times the smallest encoded size of an
  element (24 bytes for a NURBS control point, 8 for a knot, 40 bytes for a polyline vertex, 16 for a mesh vertex, 12 for a triangle, 25 for a curve,
  34 for a loop, 42 for a region part) must fit in the remaining input. Both are checked before
  memory for the array is reserved, so one reservation is at most 3.6 times the remaining input
  (a curve takes 88 bytes in memory and at least 25 encoded; every other element takes at most
  1.2 times its encoded size). The reservation is fallible: if the memory is not available, on
  any target, decoding fails with `geom.decode.invalid` instead of aborting.
- Values nest at most three levels (region, loop, curve); the depth must not exceed
  `nesting_depth`. The decoder does not recurse beyond that. The depth is counted from 1 for the
  value of each call, whatever the nesting of the document that holds the bytes.

Decoding a valid value takes time linear in its length and a peak of memory of at most about 15
times its length: measured, 2 times for a polyline, 4.4 times for a region of one-circle parts,
8.4 times for a loop of segments and 14.8 times for a loop of circles, the smallest curve. The
peak consists of the decoded curves, the pieces from which the area of a loop is computed, and
the second encoding of the canonical check. `Limits::memory_bytes` is not consulted; a caller
that enforces a memory budget bounds `input_bytes` by a fifteenth of it.

`decode` rejects bytes after the value; `decode_prefix` returns them, for values stored one after
another. Every failure is `geom.decode.invalid` with the details `offset` (the byte position of the
problem) and `reason`. When a value's constructor rejects it, `cause` holds the constructor's error
code (for example `geom.segment.zero_length`). The decoder never panics.

## Versioning

The version in the header of a value is authoritative for that value: a reader decodes by it,
whatever version a package descriptor names for the field, and a descriptor that names another
version than the header is a defect of the writer. Values of different versions may follow one
another in a stream, since each top-level value has its own header; inside one value everything
is of the version of its header.

Values have no length prefix, so a reader cannot skip a value of a kind it does not know: an
unknown tag inside a loop or region makes the whole value unreadable for that reader
([ADR-0013](../adr/0013-geometry-encoding.md)). That is why a new kind needs a new version.

Version 2 adds the NURBS curve kind and changes nothing else: every value that version 1 can hold
has the same bytes in both, and the writer uses version 1 for it, so the canonical form stays
unique (a value without a NURBS curve under a version 2 header is rejected as not canonical). A
reader of version 1 only rejects version 2 values as a whole, as it would any newer version.
Loops and regions hold no NURBS curve yet, so only a curve value is ever written in version 2.
A writer names the version of each stored value in the descriptor of its field
(`GeomCodec::encoding`).

The version names the whole layout. Any change that alters the bytes of a value that version 1
can hold, or the meaning of those bytes, needs a new version: a new kind tag, a new field, a
changed field order or width, or a new canonical rule. A decoder accepts only the versions it
implements and rejects every other version with `geom.decode.invalid`; a decoder of a version
rejects every tag that version does not assign. A new version comes with its own golden vectors
(`crates/arc-geom/tests/encoding_golden.rs`) beside those of the versions still read, and with a
migration of stored values when version 1 is no longer written.

## Example

The segment from (1, 2) to (3, −4):

```text
41 52 43 47              magic "ARCG"
01 00                    version 1
10                       tag: segment
00 00 00 00 00 00 f0 3f  start.x =  1.0
00 00 00 00 00 00 00 40  start.y =  2.0
00 00 00 00 00 00 08 40  end.x   =  3.0
00 00 00 00 00 00 10 c0  end.y   = -4.0
```

## Testing

Property tests check, for every kind with moderate and extreme magnitudes, that decoding an
encoding returns the value, that equal values (including values that differ only in the sign of
zero) give equal bytes, that a change of the byte at every position, every truncation and an
extension of a valid encoding is rejected or decodes to a value that encodes to exactly the
changed bytes, and that arbitrary bytes never panic. The values include chord arcs alone, in
loops and in reversed loops, and regions whose loops of arcs of a radius up to 1e12 the region
constructor turned. NURBS curves of degrees 1 to 5 with rational weights are checked to round-trip.
Golden vectors fix the bytes of one value of each kind, the NURBS curve in version 2. The fuzz target
`decode` in `crates/arc-geom/fuzz` decodes arbitrary input as every kind, whole and as a prefix,
with the strict profile and a profile of small limits, and checks the same re-encoding property.
