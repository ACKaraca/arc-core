# 0014. Hand-written versioned mesh encoding with optional quantization

- Status: Accepted
- Issue: [arc-core#32](https://github.com/ACKaraca/arc-core/issues/32)

## Context

Documents store triangle meshes as assets, and render and exchange paths move them between
processes. The encoding is a long-lived contract: it must be readable without Arc from its
documentation, deterministic (asset hashes identify identical meshes), exact when asked
(positions are `f64` model coordinates) and safe against hostile input, with every index checked
against the vertex count (CORE-MESH-002). Meshes dominate the size of many documents, so a
smaller lossy form at a stated error bound is wanted for display data.
[ADR-0013](0013-geometry-encoding.md) made the same decision for `arc-geom` values.

## Decision

- `arc-mesh` writes and reads its own binary layout, `arc.mesh` version 1, documented byte by
  byte in [mesh-encoding.md](../architecture/mesh-encoding.md): a magic and a version, then
  positions, triangles, channels in key order and face groups, with fixed-width little-endian
  fields and 64-bit counts. No serialization library defines it.
- Storage is exact unless the caller gives an error bound. Positions are then quantized to a grid
  over the bounding box and normals by octahedral mapping; the encoder takes the smallest bit
  width at which the error measured on the decoded values meets the bound, stores the bound and
  the bit width, and stores a section exactly when no bit width meets its bound. Encoding never
  fails because a bound is too tight.
- The decoder works on a byte slice with an `arc_base::Limits` profile, checks every count against
  the limit and the remaining input before allocating, and builds the mesh only through the
  validating builder.
- Any change to the bytes of a mesh or their meaning is a new version; a decoder rejects versions
  and channel kinds it does not implement. Golden vectors fix the bytes.

## Consequences

- Third parties can read and write meshes from the documentation alone, and a change of the
  layout is caught by the golden vectors.
- The quantization formulas, including the order of floating-point operations in decoding, are
  part of the contract: a reader must reproduce them to stay within the stored bound.
- An exact encoding is unique; a quantized one is deterministic for a mesh and bounds but is not
  reproduced by decoding and encoding again.
- The format has no entropy coding or index compression; assets rely on the package's
  compression for that.
- The encoder and decoder are maintained by hand beside the type; a new channel kind or section
  needs code in both, documentation, a version and golden vectors.

## Alternatives considered

- **A serde binary format.** Little code, but the bytes would follow a library's conventions
  rather than a documented contract, and quantization would still need a hand-written layer.
- **A mesh compression library.** Smaller output through connectivity and entropy coding, but a
  native or large dependency in a WebAssembly-set crate, no control over the error bound's
  definition, and a decoder whose hostile-input behavior Arc would not own.
- **Failing when a bound cannot be met.** Rejected because exact storage always satisfies the
  bound; callers that need to know read the mode flags.
- **Fixed 16-bit quantization.** Simpler, but the error would depend on the size of the mesh
  instead of the caller's stated bound.

## Reopen when

Mesh assets remain the dominant part of package size after package compression, or a third-party
reader cannot reproduce decoded values within the stored bounds from the documentation.
