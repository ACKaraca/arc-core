# Documentation map

Every document in this repository, one line each. Start with the architecture overview, then read
the document for the area you work on and the specification of the crate you change.

## Architecture

| Document | Content |
|---|---|
| [overview.md](architecture/overview.md) | Layers, crate map, dependency rules and their enforcement, headless and WebAssembly constraints, platform targets, feature flags, threading, errors, versioning, what belongs in arc-core |
| [document-model.md](architecture/document-model.md) | Objects, identifiers, properties, references, transactions, journal, snapshots, dependency graph, events, validation, migration |
| [document-package.md](architecture/document-package.md) | The single-file SQLite package: schema, encoding, assets, recovery, saving, locking, compatibility |
| [commands-and-automation.md](architecture/commands-and-automation.md) | Command specification, registry, command line, macros, Arc Automation Protocol, Model Context Protocol, headless CLI, confined file access, Python |
| [geometry.md](architecture/geometry.md) | Geometry tiers, tolerances, curves and regions, meshes, the B-rep API and naming, kernel isolation, constraint solver, expressions |
| [geometry-encoding.md](architecture/geometry-encoding.md) | The versioned canonical byte layout of stored `arc-geom` values, its limits and versioning rule |
| [mesh-encoding.md](architecture/mesh-encoding.md) | The versioned byte layout of stored `arc-mesh` triangle meshes, its quantization, limits and versioning rule |
| [stroke-font-format.md](architecture/stroke-font-format.md) | The versioned text format of Arc stroke fonts, its limits and the shipped font's coverage |
| [journal-record.md](architecture/journal-record.md) | The versioned canonical byte layout of operation journal records and its decoding rules |
| [drafting-and-views.md](architecture/drafting-and-views.md) | The 2D drafting model, plotting and derived views of 3D models |
| [rendering.md](architecture/rendering.md) | The viewport renderer and the Arc Scene contract |
| [ui-kit.md](architecture/ui-kit.md) | Widgets, workspace, localization, accessibility, settings, shared interaction standards, browser and tablet or phone targets |
| [interchange.md](architecture/interchange.md) | The `arc-io-*` family, per-format choices, hostile-input limits, loss reports, open readers, the Arc exchange package, licensed bridges, corpora |
| [extensibility.md](architecture/extensibility.md) | Node graphs, WebAssembly plug-ins and what is not supported |
| [technology-baseline.md](architecture/technology-baseline.md) | Third-party choices with reasons, licenses, isolation and replacement conditions |

## Crates

| Document | Content |
|---|---|
| [crates/README.md](crates/README.md) | Index of crate specifications and the capability table legend |
| [crates/arc-base.md](crates/arc-base.md) | L0 base types |
| [crates/arc-geom.md](crates/arc-geom.md) | L1 analytic geometry |
| [crates/arc-mesh.md](crates/arc-mesh.md) | L1 meshes and subdivision |
| [crates/arc-brep.md](crates/arc-brep.md) | L2 boundary representation |
| [crates/arc-solve.md](crates/arc-solve.md) | L2 constraints and expressions |
| [crates/arc-doc.md](crates/arc-doc.md) | L3 document store and package |
| [crates/arc-command.md](crates/arc-command.md) | L3 commands and automation |
| [crates/arc-draft.md](crates/arc-draft.md) | L4 drafting model |
| [crates/arc-view.md](crates/arc-view.md) | L4 derived views |
| [crates/arc-scene.md](crates/arc-scene.md) | L4 scene description |
| [crates/arc-nodes.md](crates/arc-nodes.md) | L4 node graphs |
| [crates/arc-io.md](crates/arc-io.md) | L4 interchange format family |
| [crates/arc-hub-api.md](crates/arc-hub-api.md) | L4 Hub client |
| [crates/arc-gpu.md](crates/arc-gpu.md) | L5 viewport renderer |
| [crates/arc-ui.md](crates/arc-ui.md) | L6 widget kit |
| [crates/arc-app.md](crates/arc-app.md) | L6 application shell |

## Process

| Document | Content |
|---|---|
| [testing.md](testing.md) | Test kinds, budgets, CI gate, pull request requirements |
| [roadmap.md](roadmap.md) | Foundation milestones mapped to suite phases with exit criteria |
| [glossary.md](glossary.md) | Vocabulary |

## Decisions

| Document | Content |
|---|---|
| [adr/README.md](adr/README.md) | Decision index, process and template |
| [adr/0001-enforced-layering.md](adr/0001-enforced-layering.md) | Enforced layering with a tested rule table |
| [adr/0002-persistent-identifiers-and-handles.md](adr/0002-persistent-identifiers-and-handles.md) | UUIDv7 persistent identifiers and generational handles |
| [adr/0003-journal-records-and-structural-sharing.md](adr/0003-journal-records-and-structural-sharing.md) | Field-level journal records over chunked structural sharing |
| [adr/0004-sqlite-package-layout-and-save.md](adr/0004-sqlite-package-layout-and-save.md) | SQLite package layout, recovery journal and two save strategies |
| [adr/0005-payload-encoding-with-embedded-descriptors.md](adr/0005-payload-encoding-with-embedded-descriptors.md) | Postcard payloads with embedded field descriptors |
| [adr/0006-kernel-binding-and-isolation.md](adr/0006-kernel-binding-and-isolation.md) | Kernel binding through an owned C++ shim, hostile input out of process |
| [adr/0007-arc-owned-constraint-solver.md](adr/0007-arc-owned-constraint-solver.md) | Arc-owned hybrid constraint solver |
| [adr/0008-native-subdivision-surfaces.md](adr/0008-native-subdivision-surfaces.md) | Native Catmull–Clark subdivision following published rules |
| [adr/0009-text-stack-and-drafting-fonts.md](adr/0009-text-stack-and-drafting-fonts.md) | Text stack and Arc-owned single-stroke drafting fonts |
| [adr/0010-automation-protocol-transport.md](adr/0010-automation-protocol-transport.md) | Automation protocol transport |
| [adr/0011-exchange-package-container.md](adr/0011-exchange-package-container.md) | Exchange package container: ZIP with JSON Lines and binary buffers |
| [adr/0012-opennurbs-ffi-crate.md](adr/0012-opennurbs-ffi-crate.md) | Rhino 3DM reading through an openNURBS FFI crate in the worker |
| [adr/0013-geometry-encoding.md](adr/0013-geometry-encoding.md) | Hand-written versioned geometry encoding |
| [adr/0015-stroke-font-format.md](adr/0015-stroke-font-format.md) | Versioned text format for Arc stroke fonts |

## Repository root

| Document | Content |
|---|---|
| [README.md](../README.md) | What arc-core is, layers, crates, rules products rely on |
| [AGENTS.md](../AGENTS.md) | Instructions for automated coding agents |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | How to contribute |
| [SECURITY.md](../SECURITY.md) | Reporting vulnerabilities |
