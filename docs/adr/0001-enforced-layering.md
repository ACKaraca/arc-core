# 0001. Enforced layering with a tested rule table

- Status: Accepted

## Context

Eight products and a server build on `arc-core`. The headless parts must run in CLIs, Hub workers,
tests and, for a subset, the browser. Layer rules that are only written down erode: one convenient
import of a UI type into a document crate is enough to break every headless consumer. The suite
decision on topology sets the layer numbers; this repository must make them hold.

## Decision

- The layer of every workspace crate, the allowed same-layer edges, the FFI crates, the WebAssembly
  set and the allowed third-party dependencies are recorded in one table, `xtask/layers.toml`.
- `cargo xtask layers` evaluates `cargo metadata` against the table with a pure rule function and
  fails on: upward edges, unlisted same-layer edges, UI, windowing or GPU dependencies in L0–L4,
  `unsafe` outside FFI crates, dependencies on application crates, non-wasm dependencies in the
  WebAssembly set, and third-party crates absent from the technology baseline.
- `cargo xtask wasm` builds the WebAssembly set for `wasm32-unknown-unknown`.
- Both run in the single CI gate (`cargo xtask ci`). The rule function has its own unit tests,
  including one fixture per rule that must fail.
- Changing the table requires an ADR.

## Consequences

- Layer violations are found in the pull request that introduces them, with the offending edge
  named.
- Adding a dependency is a deliberate act that updates the baseline document.
- Some convenient designs are ruled out (for example, document types that hold GPU buffers); render
  state must be derived through extractors.

## Alternatives considered

- **Documentation and review only.** Cheaper, but review misses transitive edges and drift
  accumulates silently.
- **Separate workspaces per layer.** Enforces direction mechanically but makes cross-layer changes
  and local development much heavier, and does not catch forbidden third-party crates.
- **`cargo deny` bans alone.** Handles third-party crates but cannot express workspace layer order or
  per-crate exceptions.

## Reopen when

The table needs exceptions in more than three crates for the same reason, which would indicate that
the layer boundaries themselves are wrong.

## Amendment: WebAssembly deny list

The layer table gains a list, `wasm_deny`: crates that cannot build for `wasm32-unknown-unknown`
(native database, compression, C++ bridge, async runtime, network and crash-reporting crates). The
rule function fails when a crate of the `wasm` set reaches one of them through normal dependencies
and names the chain, so the violation is found by `cargo xtask layers` and not only by a failing
`cargo xtask wasm` build. Dev-dependencies are not followed. Optional dependencies that a crate's default
features leave off are not followed either: the dependency resolution covers the whole workspace,
so a native application that turns such a feature on would otherwise fail the rule for the crates
below it. `cargo xtask wasm` builds the set with default features and finds a real use.

## Amendment: Sample set of the reference shell

The sample object types and commands of the reference shell move from the library target of the
application package `apps/arc-shell-cli` into the library crate `crates/arc-shell-sample`, so that
the windowed reference shell can use them too: the table forbids a dependency on an application
crate. The crate is classed `layer(4)`. It depends only on L0–L3 crates (`arc-base`, `arc-doc`,
`arc-command`), has no windowing, GPU or UI dependency, and its types and commands are a small
model in the same sense as the other L4 crates, so the existing class fits and the rule function
needs no new class. It is not in the WebAssembly set because its `product` function uses the
headless runner, which opens packages. Products never depend on it: it exists for the reference
shells and their tests. The rejected alternative, a new class for sample code, would have changed
the rule function for a single crate.

## Amendment: Mapping crate between a format crate and the drafting model

`arc-draft-dxf` maps the neutral drawing model of `arc-io-dxf` to the drafting model of
`arc-draft` and back, and provides the import and export commands. It is classed `layer(4)` and
has two same-layer edges, `arc-draft-dxf` to `arc-draft` and `arc-draft-dxf` to `arc-io-dxf`;
every other dependency (`arc-base`, `arc-geom`, `arc-doc`, `arc-command`) is a lower layer. It
is in the WebAssembly set, and its only file access goes through `arc_command::Context`.

The two edges are the reason for the crate. The invariants forbid the other placements: product
mapping never enters an `arc-io-*` crate, which also keeps `arc-io-dxf` free of a dependency on
`arc-draft`, and `arc-draft` does not depend on a format crate. A product that needs a different
mapping (another fidelity, product object types) depends on `arc-io-dxf` and writes its own; this
crate is the shared mapping onto the drafting types that every product stores. The edges follow
the precedent of `arc-io-pdf` to `arc-draft`, whose input is a drafting type.

The rejected alternative is a feature or module of `arc-io-dxf` that depends on `arc-draft`: it
would add the same same-layer edge, break the invariant, and put every future format crate's
drafting mapping into the format crate.

## Amendment: Test-only edge from the plot crate to the DXF mapping

`arc-io-pdf` gains a same-layer edge to `arc-draft-dxf`. It is used only by a development
dependency: the scenario test that imports a DXF file and plots it to PDF, SVG and PNG. The layer
tool does not distinguish development dependencies on same-layer crates, so the edge is recorded in
the table. No production code of `arc-io-pdf` may use `arc-draft-dxf`.

## Amendment: Shared bounded ZIP and XML readers

`arc-io-common` is added at layer 4. It holds the bounded ZIP archive reader and the bounded XML
pull reader that the ZIP- and XML-based format crates share (capability CORE-IO-007). It depends on
`arc-base` and on the third-party crates `miniz_oxide` and `quick-xml`, and it is in the
WebAssembly set. It has no same-layer edge yet: each format crate that reads ZIP or XML adds a
same-layer edge from itself to `arc-io-common` in the table when it starts using the crate.
`arc-io-ifc` (CORE-IO-014) is the first such crate and records the edge `arc-io-ifc -> arc-io-common`.

The crate is needed because the format crates may not depend on each other (the family's crates
are peers, and a format crate that read another format's container would inherit that crate's
model and limits), and the specification assigns the bounded readers to the family as a whole.
Placing the readers in `arc-base` was rejected: it would move two third-party parsers into the
foundation that every crate depends on, although only the format family needs them. Placing them in
`arc-io-exchange` was rejected for the same reason as a format-to-format edge. Every format crate
that needs the readers keeps the same behavior and the same error codes by using one crate.

## Amendment: DWG reader over a wrapped library

`arc-io-dwg` is classed `layer(4)` and has one same-layer edge, `arc-io-dwg` to `arc-io-dxf`, in
addition to its lower-layer dependencies (`arc-base`). It is in the WebAssembly set.

The edge is needed because both crates produce one neutral drawing model: the specification requires
DWG reading to return the model of the DXF reader, with its header, table, block, entity and loss
report types, and to accept the DXF reader's options. The mapping from the wrapped library to that
model belongs to the DWG crate, since the model and the reading options are the DXF crate's
definitions. The invariant that format crates expose neutral models and never take product mapping
is kept: the DWG crate maps to the neutral model and does not depend on any product crate.

The wrapped library `acadrust` (MPL-2.0, unmodified) is a third-party dependency of this crate only.
No other workspace crate depends on it, and its types appear in no public signature of the crate.

The rejected alternatives are: placing the DWG reader in `arc-io-dxf`, which would make the DXF
crate, and every crate above it, depend on a wrapped third-party library and would widen the
WebAssembly set's third-party surface for a format that is only read in phase one; and a separate
neutral model crate below both readers, which adds a crate and a layer for one shared type set that
the DXF crate already defines.
