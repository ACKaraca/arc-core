# Crate specifications

One specification per crate of the layer table; the `arc-io-*` family shares one document. Each
specification states the crate's purpose, what it owns, what stays in products, key concepts,
dependencies, test strategy and its capabilities. The last column counts the capability rows of
each specification; the total is 528.

| Crate | Layer | Area token | Specification | Capabilities |
|---|---|---|---|---|
| `arc-base` | L0 | `BASE` | [arc-base.md](arc-base.md) | 24 |
| `arc-geom` | L1 | `GEOM` | [arc-geom.md](arc-geom.md) | 32 |
| `arc-mesh` | L1 | `MESH` | [arc-mesh.md](arc-mesh.md) | 25 |
| `arc-brep` | L2 | `BREP` | [arc-brep.md](arc-brep.md) | 34 |
| `arc-solve` | L2 | `SOLVE` | [arc-solve.md](arc-solve.md) | 24 |
| `arc-doc` | L3 | `DOC` | [arc-doc.md](arc-doc.md) | 52 |
| `arc-command` | L3 | `CMD` | [arc-command.md](arc-command.md) | 38 |
| `arc-draft` | L4 | `DRAFT` | [arc-draft.md](arc-draft.md) | 36 |
| `arc-view` | L4 | `VIEW` | [arc-view.md](arc-view.md) | 22 |
| `arc-scene` | L4 | `SCENE` | [arc-scene.md](arc-scene.md) | 25 |
| `arc-nodes` | L4 | `NODES` | [arc-nodes.md](arc-nodes.md) | 23 |
| `arc-io-*` | L4 | `IO` | [arc-io.md](arc-io.md) | 55 |
| `arc-draft-dxf` | L4 | none | [arc-draft-dxf.md](arc-draft-dxf.md) | 0 |
| `arc-hub-api` | L4 | `HUBAPI` | [arc-hub-api.md](arc-hub-api.md) | 21 |
| `arc-gpu` | L5 | `GPU` | [arc-gpu.md](arc-gpu.md) | 40 |
| `arc-ui` | L6 | `UI` | [arc-ui.md](arc-ui.md) | 41 |
| `arc-app` | L6 | `APP` | [arc-app.md](arc-app.md) | 36 |

## Capability tables

Each specification ends with capability tables: `ID | Capability | Behavior | Tier | Phase`.

| Column | Meaning |
|---|---|
| ID | `CORE-<AREA>-NNN`. Identifiers are permanent; a withdrawn capability keeps its number and is marked withdrawn |
| Capability | Short name |
| Behavior | Observable behavior precise enough to implement and test |
| Tier | **T1 Core**: required for first production use. **T2 Professional**: required for parity with established professional tools. **T3 Advanced**: specialized or differentiating |
| Phase | Suite phase (0–5) in which the capability is planned; see the [roadmap](../roadmap.md) |

Implementation status is tracked in GitHub issues and the project board, never in these tables.
Issues and pull requests reference capability identifiers in their scope.
