# 0006. Kernel binding through an owned C++ shim, with hostile input out of process

- Status: Accepted

## Context

The suite decision places exact B-rep modeling behind the Arc-owned `arc-brep` API with Open
CASCADE as the first implementation, dynamically linked under LGPL-2.1. Open points: how the binding
is built, how C++ failures are contained, how hostile STEP files are handled, and how upgrades are
validated.

## Decision

- One FFI crate, `arc-brep-occt`, private to `arc-brep`, contains an Arc-owned C++ shim that exposes
  a narrow handle-based API, bound with `cxx`. Only operations used by `arc-brep` are bound.
- The shim catches every C++ exception and converts it to an error code; no exception crosses into
  Rust. All results are validated before they become `Shape` values.
- The kernel is linked dynamically. The pinned release is built by `cargo xtask occt` with a fixed
  configuration into per-platform prebuilt artifacts identified by checksum; CI and developers use
  the artifacts.
- STEP and IGES import always run in `arc-worker`. Modeling operations run in-process; batch and Hub
  jobs can request isolated execution in `arc-worker`.
- Kernel upgrades are single pull requests gated by the golden-model regression corpus (volumes,
  areas, bounding boxes, topology counts, validity and naming survival per model).

## Consequences

- Products never see kernel types; the native path can replace operation families one at a time.
- Exchanging shapes with the worker costs a serialization round trip; acceptable for imports and
  batch jobs, not used for interactive modeling.
- A native fault inside an in-process modeling operation can still end the process; the recovery
  journal bounds the loss, and isolated mode is available where that risk is unacceptable.
- Packaging includes the kernel libraries and license texts on every platform.

## Alternatives considered

- **An existing community binding crate.** Covers a subset with an opinionated API and a single
  maintainer; adopting it would still require owning the binding.
- **Static linking.** LGPL-2.1 would require shipping relinkable objects, which is impractical for a
  Rust binary.
- **All kernel calls out of process.** Strong containment, but interactive modeling latency and
  serialization costs are too high.
- **Generated bindings for the full kernel API.** Large unsafe surface and frequent breakage on
  upgrades.

## Reopen when

In-process kernel faults appear in crash reports at a rate that the recovery journal does not
adequately cover, or a pure-Rust implementation passes the golden-model corpus for an operation
family.
