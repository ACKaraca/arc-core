# Architecture decision records

Decisions specific to `arc-core`. Suite-wide decisions (repository topology, UI toolkit, geometry
tiers, document model, package, command model, regeneration, units, interoperability, rendering,
collaboration, extensibility, licensing, assistance, robustness, localization, exchange connectors and licensed bridges, web and mobile targets) are recorded in
[arc-suite](https://github.com/ACKaraca/arc-suite/tree/main/docs/adr) and are not restated here.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-enforced-layering.md) | Enforced layering with a tested rule table | Accepted |
| [0002](0002-persistent-identifiers-and-handles.md) | UUIDv7 persistent identifiers and generational handles | Accepted |
| [0003](0003-journal-records-and-structural-sharing.md) | Field-level journal records over chunked structural sharing | Accepted |
| [0004](0004-sqlite-package-layout-and-save.md) | SQLite package layout, recovery journal and two save strategies | Accepted |
| [0005](0005-payload-encoding-with-embedded-descriptors.md) | Postcard payloads with embedded field descriptors | Accepted |
| [0006](0006-kernel-binding-and-isolation.md) | Kernel binding through an owned C++ shim, with hostile input out of process | Accepted |
| [0007](0007-arc-owned-constraint-solver.md) | Arc-owned hybrid constraint solver | Accepted |
| [0008](0008-native-subdivision-surfaces.md) | Native Catmull–Clark subdivision following published rules | Accepted |
| [0009](0009-text-stack-and-drafting-fonts.md) | Text stack and Arc-owned single-stroke drafting fonts | Accepted |
| [0010](0010-automation-protocol-transport.md) | Automation protocol transport: newline-delimited JSON-RPC over stdio and user-scoped local sockets | Accepted |
| [0011](0011-exchange-package-container.md) | Exchange package container: ZIP with JSON Lines and binary buffers | Accepted |
| [0012](0012-opennurbs-ffi-crate.md) | Rhino 3DM reading through an openNURBS FFI crate in the worker | Accepted |

## Process

- An ADR is required for a change to the layer table, an FFI crate, the document package format,
  the automation protocol, the Arc Scene contract, the plug-in interface, or any public interface
  that products must migrate to.
- Number ADRs sequentially; never reuse a number. A replaced decision gets status
  `Superseded by NNNN` and stays in the index.
- An ADR is proposed in a pull request that also updates the documents describing the affected
  behavior.

## Template

```markdown
# NNNN. Title in sentence case

- Status: Proposed | Accepted | Superseded by NNNN
- Issue: link to the issue that motivated the decision

## Context

The forces at play, the requirement, and what is already decided elsewhere.

## Decision

What we do, stated so that it can be checked.

## Consequences

What becomes easier, what becomes harder, what must now be maintained.

## Alternatives considered

Each alternative with the reason it was not chosen.

## Reopen when

The observable condition that would make us revisit the decision.
```
