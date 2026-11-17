# 0013. Hand-written versioned geometry encoding

- Status: Accepted
- Issue: [arc-core#27](https://github.com/ACKaraca/arc-core/issues/27)

## Context

Documents store geometry values (curves, loops, regions, bounding boxes, view frames, triangle
meshes) inside object payloads. [ADR-0005](0005-payload-encoding-with-embedded-descriptors.md)
leaves the geometry encoding to the owning crate and requires the descriptor to name it and its
version. The encoding is a long-lived contract: it must be readable without Arc from its
documentation, deterministic (payload hashes identify identical objects), exact (coordinates
round-trip bit for bit) and safe against hostile input, with every failure reported as
`geom.decode.invalid` (CORE-GEOM-032).

## Decision

- `arc-geom` writes and reads its own binary layout, `arc.geom` version 1, documented byte by
  byte in [geometry-encoding.md](../architecture/geometry-encoding.md): a magic and a version, a
  one-byte kind tag per value, fields in a fixed order as little-endian binary64 numbers, 64-bit
  counts, one-byte flags and 32-bit triangle indices. No serialization library defines it.
- The form is canonical and unique: −0.0 is written as +0.0, equal values give equal bytes, and
  the decoder accepts only bytes that encoding the decoded value reproduces exactly.
- The decoder works on a byte slice with an `arc_base::Limits` profile, checks every count against
  the limit and the remaining input before allocating, nests at most three levels, and builds
  values only through their validating constructors.
- Any change to the bytes of a value or their meaning is a new version; a decoder rejects
  versions and kind tags it does not implement. Golden vectors in the test suite fix the bytes of
  each kind.

## Consequences

- Third parties can read and write geometry from the documentation alone, and a change of the
  layout is caught by the golden vectors.
- New kinds (NURBS, 3D curves, surfaces) take tags from reserved ranges and need a version bump,
  documentation and golden vectors.
- The encoder and decoder are maintained by hand beside the types; each new field needs code in
  both and a property test.
- Decoding costs a second encoding pass for the canonical check, linear in the input.

## Alternatives considered

- **A serde binary format (postcard, bincode).** Little code, but the bytes would follow the
  library's evolving conventions and the Rust field layout, not a documented contract;
  canonicity (the sign of zero, normalized angles) and allocation bounds would depend on the
  library's behavior.
- **A text form (JSON).** Readable, but larger and slower, with exactness depending on the number
  formatter and parser, and with no single canonical spelling of a number.

## Reopen when

A third-party reader cannot implement the layout from the documentation, or decoding geometry
becomes a measured bottleneck in loading reference models.
