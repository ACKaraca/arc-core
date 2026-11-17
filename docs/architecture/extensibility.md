# Extensibility

How products and users extend Arc beyond its built-in commands. There are three layers, all built
on the command model: scripts through the Arc Automation Protocol (see
[commands and automation](commands-and-automation.md#scripting-clients)), visual node graphs
(`arc-nodes`), and sandboxed WebAssembly plug-ins. This document specifies the last two.

## Node graphs (arc-nodes)

One engine and one editor serve parametric design, procedural modeling, material graphs,
cross-section components and automation rules. Products register node kinds and socket types; the
engine, file form and editor are shared.

### Graph model

| Element | Content |
|---|---|
| Graph | Identifier, kind (the registry namespace it uses), nodes, links, exposed inputs and outputs, frames and comments |
| Node | Local identifier (stable within the graph), node kind key and version, parameter values, editor position, enabled flag |
| Socket | Name, socket type, direction, arity (single or list), default value, optional flag |
| Link | From (node, output socket) to (node, input socket) |
| Subgraph | A graph used as a node: its exposed inputs and outputs become sockets |

### Typed sockets

- Socket types are registered in a type registry: core types (`bool`, `int`, `real`, quantity with
  dimension, `string`, `enum`, `point`, `vector`, `transform`, `color`, `list<T>`, `object reference`)
  and types registered by crates and products (curve, region, mesh, solid, material, toolpath).
  `arc-nodes` itself does not link geometry crates; geometry socket types come from registrations.
- A link is valid when the output type equals the input type, or a registered implicit conversion
  exists (integer to real, single to list, region to curve list). Dimensionally inconsistent
  quantity links are rejected in the editor and on load.
- List inputs fed a list automatically map a single-valued node over the list (with a documented
  matching rule for multiple list inputs: shortest, longest or cross product, chosen per node).

### Evaluation

- Evaluation is demand-driven: requesting an output evaluates only the nodes it depends on.
  Independent branches run in parallel on the worker pool.
- Node functions are pure: outputs depend only on inputs and parameters. Nodes with external effects
  are not allowed; a graph that should change a document produces a **command batch** output,
  applied by the host as one transaction group (automation rules), or produces derived data
  consumed through the dependency graph (parametric geometry).
- A graph stored in a document is a document object; each exposed output is a node of the document
  dependency graph, so graph evaluation inherits revision tagging, cancellation and failure states.
- Errors attach to the node that produced them and propagate as `blocked` downstream; partial
  results upstream of the failure stay available.
- Limits: evaluation steps, list lengths, recursion through subgraphs (depth 32) and memory are
  bounded per evaluation.

### Caching

Each node output is cached with a hash of its input values and parameters. Changing one parameter
re-evaluates only the downstream cone, and early cutoff stops propagation when an output hash is
unchanged. Caches are memory-bounded with least-recently-used eviction and are never persisted
except where a node kind declares its output an asset (a baked mesh).

### Serialization

Graphs serialize as part of their owning document object (encoded payload) and as a standalone
JSON file (`.arcgraph`) for exchange, with the schema published in `schemas/nodes/`. Each node
records its kind version; node kinds provide migrations between versions like object types do. An
unknown node kind loads as a preserved placeholder node with its sockets and parameters intact,
so graphs survive being opened by a product that lacks the kind.

### Subgraphs and graph assets

A subgraph can be defined inside a graph or published as a **graph asset**: a standalone graph with
a name, version, description, exposed sockets and an icon, stored in a library file or in a
document. References to graph assets record the asset identifier and version and the content hash
of the definition used; updating an asset is an explicit command that shows which graphs change.

## WebAssembly plug-ins

Plug-ins are WebAssembly components run by `wasmtime` using the component model. They add commands,
node kinds, import and export mappers, model-checking rules and panels built from declarative
widgets. There are no native plug-ins.

### Interface

- The host interface is defined in WIT in `wit/` as the world `arc:plugin`, versioned
  independently of crates (`arc:plugin@1.0.0`). Minor versions only add functions and types;
  a major version may change them, and the host supports the previous major for at least one suite
  release. Archived plug-in binaries are part of the host's compatibility test suite.
- Imports offered to plug-ins: read access to a document snapshot (objects, fields, properties,
  derived data the host exposes), command invocation (subject to the plug-in's grants), logging,
  progress and cancellation, a host-provided clock, declarative UI descriptions, and file handles
  passed by the host.
- Exports from plug-ins: registration of commands (with parameter schemas), node kinds, mappers and
  rules, and the functions that implement them.

### No ambient authority

A plug-in has no access to the file system, network, environment variables, processes, clock or
randomness except what the host passes explicitly: file handles chosen by the user or within
granted roots, a deterministic random seed when requested, and command invocation limited to its
granted permission classes. WASI interfaces are not linked except the subset the host implements
over these grants.

### Resource limits

| Resource | Default limit |
|---|---|
| Component size | 64 MiB |
| Linear memory per instance | 1 GiB |
| Fuel (instruction budget) per call | Set per call kind; exceeding it traps the call |
| Wall-clock time per call | 60 s, interruptible by the user |
| Call stack depth | Bounded by the engine configuration |
| Concurrent instances per plug-in | 4 |

A trap, limit violation or error ends the call; the plug-in instance is discarded and a fresh one
created for the next call.

### Transactional application

A plug-in never mutates a document directly. Its command implementations run inside the host's
command executor: the plug-in reads the transaction's view and returns operations or calls other
commands; the host validates and commits. Any failure rolls the whole transaction back, so a faulty
plug-in cannot leave a document half changed.

### Package manifest

A plug-in package is a ZIP file containing `arc-plugin.toml`, the component, Fluent catalogs, icons
and license files. The manifest declares:

| Field | Content |
|---|---|
| `id` | Reverse-domain identifier, unique |
| `name`, `version`, `authors`, `license` | SPDX license identifier required |
| `world` | The `arc:plugin` version it targets |
| `products` | Products and minimum versions it supports |
| `permissions` | Requested permission classes and their justification messages |
| `contributes` | Commands, node kinds, mappers, rules and panels it registers |
| `checksum` | SHA-256 of the component |

Installation unpacks with bounded sizes into a per-user store, verifies the checksum, and installs
atomically (an interrupted install leaves the previous state). A package is installed once per
user and enabled per product.

### Host-assigned trust tiers

The host, not the package, assigns trust:

| Tier | Assigned when | Effect |
|---|---|---|
| `bundled` | Shipped with the product | Permissions granted as declared |
| `verified` | Signed by a key the user or administrator added to the trusted key list | Declared permissions granted after one confirmation |
| `local` | Any other package | Runs with `read` only until the user grants each requested class |
| `blocked` | Listed by an administrator policy | Not loaded |

Granted permissions are shown in the plug-in manager and can be revoked at any time.

## Not supported

- **Native plug-ins** (dynamic libraries loaded into the process) are not supported: they bypass
  memory safety and the no-panic guarantee, cannot be sandboxed, and tie plug-ins to one platform
  and compiler version.
- **Embedded script interpreters** are not part of the first releases; scripting runs out of process
  through the automation protocol. Embedding is reconsidered only if out-of-process scripting
  cannot meet interactive performance needs.
- **Plug-ins that replace core subsystems** (renderer, document store, UI toolkit) are not
  supported; such changes are made in `arc-core`.
