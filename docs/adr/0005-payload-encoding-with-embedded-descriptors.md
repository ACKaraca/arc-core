# 0005. Postcard payloads with embedded field descriptors

- Status: Accepted

## Context

Object payloads must be compact (models with tens of millions of objects), fast to decode,
deterministic (payload hashes identify identical objects), evolvable by schema version, and readable
without Arc as promised by the suite's open-format principle.

## Decision

- Payloads are encoded with `postcard` from the type's fields in declared order, compressed with
  `zstd` above 4 KiB.
- Each package stores, per type and schema version present, a JSON field descriptor (names, value
  types, dimensions, optionality, enum codes, geometry encodings). The descriptor grammar is a
  published JSON Schema. Together they let a generic decoder turn any payload into JSON.
- Schema minor versions only append optional fields at the end. A reader of an older minor decodes
  the known prefix and preserves the remaining bytes; documents with newer minor types open
  read-only.
- Geometry values use encodings owned and versioned by their crate and named in the descriptor.
- Encoding is canonical: maps are written in key order, floating-point values bit-exact, no
  optional padding.

## Consequences

- Payloads are close to the size of the raw data, and decoding is a linear pass.
- The format is readable by third parties through the descriptors without access to Arc source.
- Fields cannot be removed or reordered within a major version; such changes need a major version
  and a migration.
- Every type must keep its descriptor generation in sync with its Rust definition; a test compares
  the generated descriptor with the stored fixture per version.

## Alternatives considered

- **JSON or CBOR payloads.** Self-describing, but two to five times larger and slower to decode;
  determinism of CBOR maps needs extra rules.
- **Protocol Buffers or FlatBuffers.** Good evolution rules, but a second schema language beside
  Rust types and a code generation step in every product.
- **Zero-copy archives.** Fast reads, but schema evolution is difficult; acceptable for caches only,
  not for source documents.

## Reopen when

Decoding becomes a measured bottleneck in loading reference models, or a third-party consumer cannot
decode packages with the published descriptors.

## Amendment

The field descriptor carries, for each field, everything the field declaration states: besides the
names, value types, dimensions, optionality, enum codes and geometry encodings listed above, also
the default, the valid range or length bound, indexing, the aspect and, for reference fields, the
relationship rule. The list is maintained in
[document-model.md](../architecture/document-model.md#fields-properties-and-schemas) and
[document-package.md](../architecture/document-package.md#payload-encoding). The payload encoding
is unchanged.
