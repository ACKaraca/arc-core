# Mesh encoding

The byte form in which `arc-mesh` stores indexed triangle meshes (CORE-MESH-002): encoding
`arc.mesh`, version 1. Meshes are stored as document assets in this form; the decision for a
hand-written layout with optional quantization is recorded in
[ADR-0014](../adr/0014-mesh-encoding.md). The API is `TriMesh::encode` and `TriMesh::decode`
([arc-mesh](../crates/arc-mesh.md#mesh-encoding)).

## Conventions

| Field type | Bytes | Form |
|---|---|---|
| `f64` | 8 | IEEE 754 binary64, little-endian; finite, never −0.0 |
| `f32` | 4 | IEEE 754 binary32, little-endian; finite, never −0.0 |
| `u64` | 8 | Unsigned integer, little-endian; used for counts |
| `u32` | 4 | Unsigned integer, little-endian; used for indices, slots and keys |
| `u16` | 2 | Unsigned integer, little-endian; used for the version |
| `u8` | 1 | Unsigned integer; used for kinds, bit widths and component counts |
| `flag` | 1 | 0 (false) or 1 (true); any other value is invalid |
| `point3` | 24 | `x`, `y`, `z` as `f64` |
| `code(b)` | ⌈b / 8⌉ | A quantized value of bit width `b` as an unsigned little-endian integer |

## Layout

| Section | Field | Type | Content |
|---|---|---|---|
| Header | magic | 4 bytes | `41 52 43 4D` (ASCII `ARCM`) |
| | version | `u16` | `1` |
| Positions | quantized | `flag` | 0: exact positions; 1: positions on a grid |
| | bits | `u8` | Only when quantized: the grid bit width `b`, 1 to 32 |
| | bound | `f64` | Only when quantized: the error bound the encoder was given, positive |
| | min, max | 2 × `point3` | Only when quantized: the grid box, `min ≤ max` on every axis and `max − min` finite |
| | vertices | `u64` | The vertex count `V` |
| | data | | Exact: `V` × `point3`. Quantized: `V` × three `code(b)` (x, y, z), each at most 2^b − 1 |
| Triangles | triangles | `u64` | The triangle count `T` |
| | data | `T` × 3 × `u32` | Vertex indices, each below `V` |
| Channels | channels | `u64` | The channel count, at most 64 |
| | data | | The channels, in strictly increasing key order |
| Groups | groups | `u64` | The face group count |
| | data | | The groups, in triangle order |

A quantized position coordinate is `min + (max − min) · (q / (2^b − 1))`, evaluated in binary64 in
exactly this order, per axis.

### Channels

Every channel holds one value per vertex. It starts with a kind byte and the fields of its key:

| Kind | Key | Key fields | Components |
|---|---|---|---|
| `1` | Normal | none | 3 |
| `2` | Tangent | none | 4: direction and handedness, exactly 1 or −1 |
| `3` | UV set | `set: u32` | 2 |
| `4` | Color | none | 4: red, green, blue, alpha |
| `5` | User | `length: u64`, then `length` bytes of UTF-8 (not empty); `components: u8`, 1 to 4 | as stored |

Keys order by kind, then by UV set index or by the bytes of the user name. The values follow as
`V` × components × `f32`, except for the normal channel, which has one more field before its
values:

| Field | Type | Content |
|---|---|---|
| quantized | `flag` | 0: `V` × 3 × `f32`; 1: octahedral codes |
| bits | `u8` | Only when quantized: the bit width `b`, 2 to 24 |
| bound | `f64` | Only when quantized: the angular error bound in radians the encoder was given, positive |
| data | `V` × two `code(b)` | Only when quantized: `qu`, `qv`, each at most 2^b − 2 |

A quantized normal is decoded with `n = 2^b − 2`, `u = 2·qu/n − 1`, `v = 2·qv/n − 1` and
`z = 1 − |u| − |v|`. When `z < 0`, `u` and `v` become `(1 − |v|)·sign(u)` and `(1 − |u|)·sign(v)`,
computed from the values before the change, with `sign(0) = 1`. The normal is `(u, v, z)` scaled to
unit length in binary64 and rounded to `f32`. The number of steps is even, so the six axis
directions are stored without error.

### Groups

| Field | Type | Content |
|---|---|---|
| start, end | 2 × `u32` | The triangle range `start..end`; not empty, `end ≤ T`, `start` not below the previous group's `end` |
| material | `u32` | The material slot |
| key | `u32` | The local key of the source sub-object |
| has object | `flag` | Whether a source object identifier follows |
| object | 16 bytes | Only when flagged: the `ObjectId` in the byte order of RFC 9562 |

## Quantization

Quantization is chosen by the encoder from the caller's error bounds; without bounds everything is
exact.

- **Positions.** The box is the bounding box of the positions. The encoder takes the smallest bit
  width from 1 to 32 at which the distance between every position and its decoded value, computed
  from the decoded value as the decoder computes it, is at most the bound. A quantized coordinate
  is `round((x − min) / (max − min) · (2^b − 1))`, or 0 on an axis without extent.
- **Normals.** The encoder takes the smallest bit width from 2 to 24 at which the angle between
  every normal and its decoded `f32` value is at most the bound. The octahedral coordinates of a
  unit normal are `(x, y) / (|x| + |y| + |z|)`, folded as in decoding when `z < 0`, and a code is
  `round((u + 1) / 2 · (2^b − 2))`. Decoded normals have unit length: a normal is a direction,
  and its stored length is not kept.
- **Fallback.** A section is stored exactly, which meets every bound, when no bit width meets the
  bound, when the mesh has no vertices, when the extent of the box is not representable, or when
  a normal is zero.

The stored bound is the caller's bound, not the smaller error actually reached.

## Canonical form

- The same mesh and bounds give the same bytes. The sign of zero is not part of a value: a mesh
  never holds −0.0, and the decoder rejects its bit pattern.
- Every field has one width and one accepted form (flags are 0 or 1, channels are in key order,
  codes do not exceed their range), so an exact encoding is unique: decoding it and encoding the
  result without bounds gives the same bytes.
- Decoding a quantized encoding and encoding the result with the same bounds can choose a
  different bit width, because the bounds are then measured against the decoded values.

## Limits and errors

`decode` takes an `arc_base::Limits` profile:

- The input length must not exceed `input_bytes`.
- The vertex, triangle and group counts must not exceed `array_length`, and each count times the
  encoded size of an element (24 bytes or three codes for a position, 12 for a triangle, the
  components times 4 or two codes for a channel value, 17 for a group) must fit in the remaining
  input. A user name must not exceed `string_bytes`. All are checked before anything is
  allocated, so the decoder allocates at most a small multiple of the input length (eight times
  for positions quantized to one byte per coordinate).
- The decoded parts are validated as a mesh is on construction: every index against the vertex
  count, every group against the triangle count, the tangent handedness, and the counts against
  `array_length` and 32 bits.

Bytes after the mesh are rejected. Every failure is `mesh.decode.invalid` with the details
`offset` (the byte position of the problem) and `reason`. When validation rejects the mesh,
`cause` holds its error code (for example `mesh.index.out_of_range`). The decoder never panics.

## Versioning

The version names the whole layout. Any change that alters the bytes of a mesh that version 1 can
hold, or the meaning of those bytes, needs a new version: a new section, channel kind or field, a
changed width or order, a different quantization formula or range of bit widths. A decoder accepts
only the versions it implements and rejects every other version, and every channel kind it does
not know, with `mesh.decode.invalid`. A new version comes with its own golden vectors
(`crates/arc-mesh/tests/encoding_golden.rs`) beside those of the versions still read.

## Example

One triangle with the vertices (0, 0, 0), (1, 0, 0) and (0, 2, 0), no channels and no groups:

```text
41 52 43 4d              magic "ARCM"
01 00                    version 1
00                       positions: exact
03 00 00 00 00 00 00 00  3 vertices
00 00 00 00 00 00 00 00  0.0   (9 numbers: x, y, z of each vertex)
00 00 00 00 00 00 00 00  0.0
00 00 00 00 00 00 00 00  0.0
00 00 00 00 00 00 f0 3f  1.0
00 00 00 00 00 00 00 00  0.0
00 00 00 00 00 00 00 00  0.0
00 00 00 00 00 00 00 00  0.0
00 00 00 00 00 00 00 40  2.0
00 00 00 00 00 00 00 00  0.0
01 00 00 00 00 00 00 00  1 triangle
00 00 00 00              index 0
01 00 00 00              index 1
02 00 00 00              index 2
00 00 00 00 00 00 00 00  0 channels
00 00 00 00 00 00 00 00  0 groups
```

## Testing

Property tests check that an exact encoding decodes to the mesh bit for bit and is unique, that a
quantized encoding decodes within its bounds measured on the decoded mesh, that every single-byte
change, truncation and extension of an encoding is rejected or decodes to a valid mesh, and that
arbitrary bytes never panic. Golden vectors fix the bytes of one exact and one quantized mesh.
The fuzz target `decode` in `crates/arc-mesh/fuzz` decodes arbitrary input with the strict
profile and checks that a decoded mesh encodes and decodes to itself.
