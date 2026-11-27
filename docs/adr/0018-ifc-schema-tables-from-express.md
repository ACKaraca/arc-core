# 0018. IFC schema tables generated from the published EXPRESS schemas

- Status: Proposed
- Issue: [arc-core#112](https://github.com/ACKaraca/arc-core/issues/112)

## Context

`arc-io-ifc` reads the STEP physical file into a generic model of instances, records and parameters
and knows no schema: it accepts any entity name and never checks attribute counts, types,
enumeration values, the positions of derived attributes, inverse attributes or the EXPRESS rules.
The typed views and the geometry evaluator use attribute positions written by hand, the writer emits
IFC4 only, and nothing that was read can be written back. Complete support of IFC2X3 TC1, IFC4 ADD2
TC1 and IFC4X3 ADD2 means 653, 776 and 876 entities with 1320, 1491 and 1644 explicit attributes, 365,
679 and 779 WHERE and global rules and 38, 47 and 48 functions
([ifc-schema-support.md](../architecture/ifc-schema-support.md#schema-size)). That cannot be kept
correct by hand.

The schemas are published by buildingSMART International as EXPRESS (ISO 10303-11) text. The IFC2X3
and IFC4 publications carry a notice that allows their use in software development and their
translation into another computer language with full attribution; formal IFC4.3 releases are
published under CC BY-ND 4.0. Neither license is on the suite allowlist
([dependency policy](https://github.com/ACKaraca/arc-suite/blob/main/docs/engineering/dependency-policy.md)).

`technology-baseline.md` names a third-party crate behind `arc-io-ifc`; the crate specification and
the code build the reader in the crate. This decision settles that difference.

## Decision

- `cargo xtask ifc-schema` parses the published EXPRESS schemas of IFC2X3 TC1, IFC4 ADD2 TC1 and
  IFC4X3 ADD2 and generates compact static tables, checked in under
  `crates/arc-io-ifc/src/schema/`: entities (supertype, abstract flag, explicit attributes with type,
  optionality and aggregate bounds, derived re-declarations, inverse attributes), defined,
  enumeration and select types, and in a later step the WHERE rules, global rules and functions as
  expression trees. The tables are data; no Rust type is generated per entity.
- The schema source files are not stored in any Arc repository. The generator reads them from a
  directory the maintainer supplies, verifies each against a pinned BLAKE3 digest and never
  downloads. Every generated file carries the attribution the publication asks for and has a row in
  `ATTRIBUTION.md`. This starts only after the maintainer records the license basis in the dependency
  policy.
- A schema-aware layer over the generic model resolves attributes by name, tests supertypes and
  builds inverse indexes. The views, the geometry evaluator and the writer use it instead of
  positions written by hand.
- A conformance checker reports, per rule, whether it passed, failed or was not evaluated, at three
  levels: instance structure, inverse cardinalities and uniqueness, WHERE and global rules through a
  bounded interpreter. A rule that is not evaluated is never reported as passed.
- Every file the writer produces passes the structure level by construction; the writer gains the
  schema choice IFC4, IFC4X3 ADD2 or IFC2X3.
- The reader keeps the byte span of each instance. A serializer writes unmodified instances
  byte-identical to their source and changed or new instances in the canonical form, so foreign
  content survives a round trip.
- IfcOpenShell stays an out-of-process oracle outside the gate: schema and rule validation of every
  written file, comparison of the generated tables, and geometry comparison.

## Consequences

- Completeness becomes checkable: the tables are compared with an independent reader, every written
  file is validated, and the loss report counts what is not yet evaluated.
- Products migrate to new public items of `arc-io-ifc` (the schema module, findings, the edit API,
  the schema option of the writer); the existing generic model stays.
- A schema revision is a regeneration plus review of the table diff.
- The generator, the EXPRESS parser and the rule interpreter must be maintained; the interpreter
  runs under the caller's limits.
- `technology-baseline.md` lists `arc-io-ifc` as built in the crate once this decision is accepted.

## Alternatives considered

- **One generated Rust type per entity.** About 2300 types over three schemas, long compile times and
  a larger WebAssembly build, and the generic model is still needed to preserve unknown content.
- **Wrapping a third-party IFC crate.** Usable only unmodified under its license, does not cover the
  rules, and does not follow the suite's limits model.
- **Parsing EXPRESS at run time.** Ships the schema text with every build, adds hostile-input
  surface and does not remove the license question.
- **Tables written by hand.** No proof of completeness.

## Reopen when

The maintainer does not accept the license basis for generated tables, or buildingSMART publishes the
schemas under a license on the allowlist (the source files could then be stored in the repository).
