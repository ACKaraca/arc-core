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
- Values carry no length prefix. A reader cannot skip a value of a kind it does not know, so a
  new kind is a new version and a reader of an older version rejects the whole value that
  contains it, not only the unknown part.
- The version in the header of a value is authoritative for it; the version a descriptor names
  for the field must agree. Every top-level value has a header, so a stream may mix versions at
  top-level value boundaries and nowhere else.
- The byte contract is structural. The geometric validation of loops and regions (closure within
  the default linear tolerance, orientation, nonzero area) is applied by the reader on top of it
  and is implementation-defined at its boundary; the layout document says how a writer stays
  clear of it.
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
- A document written by a newer version with a new kind is unreadable as a whole to an older
  reader of that value; partial reading of geometry is given up for a layout without redundancy,
  in which a length can never disagree with the content it prefixes.

## Alternatives considered

- **A serde binary format (postcard, bincode).** Little code, but the bytes would follow the
  library's evolving conventions and the Rust field layout, not a documented contract;
  canonicity (the sign of zero, normalized angles) and allocation bounds would depend on the
  library's behavior.
- **A text form (JSON).** Readable, but larger and slower, with exactness depending on the number
  formatter and parser, and with no single canonical spelling of a number.

- **Length-prefixed values.** A byte length before every value would let an older reader skip
  kinds it does not know. It adds a second source of truth that a decoder must check against the
  content, makes the canonical form depend on it, and a loop or region with a skipped curve is not
  a usable value anyway. Rejected; the consequence is recorded above.

## Amendment before first release

Version 1 was changed after review, before any document was stored with it, so the version
number stays 1:

- The arc of a polyline segment has its own kind, the chord arc (tag `0x18`: start, end, bulge).
  Before, it was written in center-radius form, lost its chord, and a loop holding a nearly
  straight one could not be read back.
- `decode_prefix` counts only the bytes of the value toward `input_bytes`, and memory for arrays
  is reserved fallibly.
- The structural contract and the reader's geometric validation are specified separately, and
  the rules on the header version, mixed streams, nesting depth, clockwise full circles and
  boxes with infinite corners are stated.

## Amendment: NURBS curves in version 2

The NURBS curve kind (CORE-GEOM-008) is added under the rules above, as a new version:

- Version 2 is version 1 plus the kind tag `0x19` from the range reserved for 2D curves, with
  the layout in [geometry-encoding.md](../architecture/geometry-encoding.md). Nothing else
  changes; the bytes of every value version 1 can hold are the same in both versions.
- The writer uses the lowest version that holds the value: version 1 for every value without a
  NURBS curve, so no existing byte form and no version 1 golden vector changes, and version 2
  for a value that holds one. A version 2 header around a value without a NURBS curve is not
  canonical and is rejected, so each value still has exactly one byte form. A version 1 header
  with the NURBS tag is rejected.
- The decoder reads versions 1 and 2. A reader of version 1 only rejects version 2 values as a
  whole, which is the consequence recorded above for any new kind.
- Because the version depends on the value, the codec reports it (`GeomCodec::encoding`) for the
  descriptor of the field. Loops and regions do not hold NURBS curves yet, so only curve values
  are written in version 2.
- Golden vectors fix the bytes of a NURBS curve in version 2 beside the version 1 vectors, and
  the fuzz target decodes NURBS curves.

The change is additive in the sense of these rules: it adds a version and a tag, and leaves the
meaning and the bytes of every version 1 value unchanged; no stored value needs a migration.

## Reopen when

A third-party reader cannot implement the layout from the documentation, or decoding geometry
becomes a measured bottleneck in loading reference models.
