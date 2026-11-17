# Geometry encoding

The byte form in which `arc-geom` stores geometry values (CORE-GEOM-032): encoding `arc.geom`,
version 1. Package descriptors name this encoding and its version for every geometry field
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

An encoded value is a header followed by one value.

| Offset | Field | Type | Content |
|---|---|---|---|
| 0 | magic | 4 bytes | `41 52 43 47` (ASCII `ARCG`) |
| 4 | version | `u16` | `1` |
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

Reserved: `0x18`–`0x1F` for further 2D curve kinds (NURBS, transition spirals, composite
curves), `0x60`–`0x6F` for 3D curves, `0x70`–`0x7F` for surfaces. Every tag not assigned by the
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
| Polyline | `closed: flag`, `count: u64`, then `count` vertices of `point: point2`, `bulge: f64`, `start_width: f64`, `end_width: f64` (40 bytes each) |
| Loop | `count: u64`, then `count` curve values |
| Region | `parts: u64`, then per part: the outer loop value, `holes: u64`, then `holes` loop values |
| 2D bounding box | `nonempty: flag`; when 1, `min: point2`, `max: point2` |
| 3D bounding box | `nonempty: flag`; when 1, `min: point3`, `max: point3` |
| View frame | `eye: point3`, `target: point3`, `up: vector3` (the up hint as given), `projection: u8` (0 perspective, 1 orthographic), `value: f64` (vertical field of view in radians, or view height), `near: f64`, `far: f64` |
| 2D triangle mesh | `vertices: u64`, then `vertices` × `point2`; `triangles: u64`, then `triangles` × three `u32` vertex indices |

An arc taken from a bulged polyline segment keeps the chord it was built from for evaluation; the
chord is not part of the stored value, and the arc is written in its center-radius form, as with
serde. Derived data (the basis of a view frame, bounding boxes of curves) is never stored.

## Canonical form

- Equal values give equal bytes. The sign of zero is not part of a value: −0.0 is written as
  +0.0, and the decoder rejects the bit pattern of −0.0. Every other number is written bit for
  bit, so decoding returns exactly the stored numbers.
- Decoding builds every value with its type's validating constructor, so no invalid value is
  created. It then encodes the value again and rejects the input unless the bytes are identical.
  Input that a constructor would normalize is therefore rejected rather than silently changed: an
  arc start angle or elliptical arc start parameter outside [0, 2π), a nonzero bulge or width on
  the last vertex of an open polyline, a region outer loop that is not counterclockwise or a hole
  that is not clockwise.
- A loop is validated as deserialization validates it: consecutive curves must meet within the
  default linear tolerance, and remaining gaps are kept, not closed with segments. A view frame is
  validated with the default tolerance.

## Limits and errors

`decode` and `decode_prefix` take an `arc_base::Limits` profile:

- The input length must not exceed `input_bytes`.
- Every count must not exceed `array_length`, and the count times the smallest encoded size of an
  element (40 bytes for a polyline vertex, 16 for a mesh vertex, 12 for a triangle, 25 for a curve,
  34 for a loop, 42 for a region part) must fit in the remaining input. Both are checked before
  anything is allocated, so a decoder never allocates more than a small multiple of the input
  length.
- Values nest at most three levels (region, loop, curve); the depth must not exceed
  `nesting_depth`. The decoder does not recurse beyond that.

`decode` rejects bytes after the value; `decode_prefix` returns them, for values stored one after
another. Every failure is `geom.decode.invalid` with the details `offset` (the byte position of the
problem) and `reason`. When a value's constructor rejects it, `cause` holds the constructor's error
code (for example `geom.segment.zero_length`). The decoder never panics.

## Versioning

The version names the whole layout. Any change that alters the bytes of a value that version 1
can hold, or the meaning of those bytes, needs a new version: a new kind tag, a new field, a
changed field order or width, or a new canonical rule. A decoder accepts only the versions it
implements and rejects every other version with `geom.decode.invalid`; a decoder of version 1
rejects every tag it does not know. A new version comes with its own golden vectors
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
zero) give equal bytes, that every single-byte change, truncation and extension of a valid
encoding is rejected or decodes to a value that encodes to exactly the changed bytes, and that
arbitrary bytes never panic. Golden vectors fix the bytes of one value of each kind. The fuzz
target `decode` in `crates/arc-geom/fuzz` decodes arbitrary input as every kind with the strict
profile and checks the same re-encoding property.
