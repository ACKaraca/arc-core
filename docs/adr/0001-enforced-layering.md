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
