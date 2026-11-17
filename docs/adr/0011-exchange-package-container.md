# 0011. Exchange package container: ZIP with JSON Lines and binary buffers

- Status: Accepted
- Issue: Exchange connectors and licensed bridges (suite decision
  [0017](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0017-exchange-connectors-and-licensed-bridges.md))

## Context

Connectors run inside other applications and must write and read the Arc exchange package from
whatever language and runtime the host requires, often without the ability to load a native
library. The package must carry geometry, semantics, types, parameters with units, relationships,
source identifiers and drafting views, be validated independently of Arc, and evolve without breaking
connectors already in use. The document package ([ADR-0004](0004-sqlite-package-layout-and-save.md))
is a store with a journal and is not meant for snapshots written by third parties.

## Decision

- The exchange package is a ZIP archive (`.arcx`) holding a manifest, JSON Lines tables, binary
  geometry buffers, content-addressed assets, preserved host data and a producer report. The layout,
  logical contents, validation levels and versioning rules are specified in
  [interchange.md](../architecture/interchange.md#the-arc-exchange-package).
- The schema version is `major.minor` and is independent of crate versions, the document package
  format, the protocol and the plug-in interface. Minor versions only add; readers keep unknown
  fields and refuse unknown major versions or unimplemented required extensions.
- Geometry is exchanged as Arc-neutral B-rep topology with analytic and NURBS geometry, indexed
  meshes and curves, in meters and radians with an `f64` local origin. The format crate does not
  depend on `arc-brep`; products convert.
- JSON Schemas for every table are published in `schemas/exchange/`, and an independent validator
  lives in the connector repository.

## Consequences

- Any host language with a ZIP and a JSON library can write a package; no SQLite or Arc library is
  needed in the host.
- Packages are snapshots: whole-model reads are streamed table by table, but random access to one
  element needs an index built by the reader. Large models cost more to open than a document package.
- The format is a public interface: changes need a decision record, fixtures and a coordinated
  connector release.

## Alternatives considered

- **SQLite file like the document package.** Gives random access and atomic writes, but requires a
  SQLite binding in every host, and hosts differ in which versions and extensions they allow.
- **IFC as the container.** Open and widely supported, but it cannot carry host parameter formulas,
  source identifiers for round trips, families with parameter definitions or drafting geometry
  without private extensions that would amount to this format.
- **glTF or USD with extensions.** Carry geometry well, not semantics, types and relationships.
- **A single JSON document.** Cannot stream large models and has no place for binary buffers.

## Reopen when

Measured open time of the largest realistic packages exceeds the budget set for the browser build,
or three or more connectors need random access that the tables cannot provide.
