# arc-nodes

The typed dataflow node-graph engine used for parametric design, procedural modeling, material
graphs, cross-section components and automation rules. The architecture is specified in
[extensibility.md](../architecture/extensibility.md#node-graphs-arc-nodes).

## Owns

- The graph model: nodes, sockets, links, subgraphs, frames and comments.
- The socket type registry with implicit conversions, and the node kind registry with versions and
  migrations.
- Link validation, list mapping rules and evaluation.
- Output caching with early cutoff and memory bounds.
- Integration of document-stored graphs with the `arc-doc` dependency graph.
- Serialization in documents and as `.arcgraph` JSON with a published schema.
- Graph assets and their versioned references.
- Core node kinds: math, logic, comparison, lists, strings, quantities and conversions.

## Stays in products

Domain node kinds (wall from curve, corridor component, material layer, rig constraint), socket types
for domain values, the mapping from command-batch outputs to product commands, and graph libraries
shipped with a product. The node editor widget lives in `arc-ui`.

## Key concepts and types

| Concept | Description |
|---|---|
| `Graph` | Nodes, links, exposed sockets, frames, kind namespace |
| `NodeKind` | Key, version, sockets, parameters, evaluate function, migrations |
| `SocketType` | Registered type with equality and conversions |
| `Link` | Output socket to input socket |
| `GraphAsset` | Published graph with identifier, version and content hash |
| `EvalRequest` | Outputs to compute, cancellation token, limits |
| `NodeState` | Clean, evaluating, failed, blocked, with error and timing |
| `CommandBatch` | Output type carrying command invocations for the host to apply |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-doc`, `arc-command` (command-batch outputs), `arc-solve` (expression nodes) |
| Third-party | `serde`, `serde_json`, `rayon` |

Geometry socket types are registered by crates and products; `arc-nodes` links no geometry crate.

## Test strategy

Property tests for evaluation determinism and cache correctness (evaluating with and without caches
gives identical outputs); link-validation tests for every conversion; serialization round trips and
migration fixtures; placeholder preservation tests for unknown node kinds; limit tests for list
sizes and subgraph depth; benchmarks for large graphs (10,000 nodes).

## Capabilities

### Graph model

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-NODES-001 | Graph structure | Represents graphs of nodes with typed input and output sockets connected by links, plus frames and comments. | T1 | 3 |
| CORE-NODES-002 | Socket type registry | Registers core and host socket types with equality and implicit conversions. | T1 | 3 |
| CORE-NODES-003 | Link validation | Accepts links only between compatible types or registered conversions; quantity sockets reject dimensionally inconsistent links. | T1 | 3 |
| CORE-NODES-004 | Node kind registry | Registers node kinds with key, version, sockets, parameters and evaluate function from crates, products and plug-ins. | T1 | 3 |
| CORE-NODES-005 | Exposed inputs and outputs | Exposes chosen sockets as the graph's interface, used by subgraphs, documents and automation. | T1 | 3 |
| CORE-NODES-006 | Core node library | Provides math, logic, comparison, list, string, quantity and conversion nodes. | T1 | 3 |

### Evaluation and caching

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-NODES-007 | Demand-driven evaluation | Evaluates only nodes required by requested outputs, in parallel where independent. | T1 | 3 |
| CORE-NODES-008 | Pure node functions | Node functions receive only inputs and parameters; nodes cannot access documents, files or the network directly. | T1 | 3 |
| CORE-NODES-009 | List mapping | Maps single-valued nodes over list inputs using the node's declared matching rule (shortest, longest, cross product). | T1 | 3 |
| CORE-NODES-010 | Output caching | Caches outputs keyed by input and parameter hashes; a parameter change re-evaluates only its downstream cone, stopping where outputs are unchanged. | T1 | 3 |
| CORE-NODES-011 | Cache bounds | Evicts least-recently-used cached outputs beyond a memory budget without changing results. | T2 | 3 |
| CORE-NODES-012 | Errors and blocked states | Attaches errors to failing nodes and marks downstream nodes blocked while upstream results remain available. | T1 | 3 |
| CORE-NODES-013 | Evaluation limits | Bounds evaluation steps, list lengths, subgraph depth and memory; exceeding a limit fails the request with the node named. | T1 | 3 |
| CORE-NODES-014 | Cancellation | Cancels evaluation when inputs change or the requester cancels, discarding partial results. | T1 | 3 |
| CORE-NODES-015 | Document integration | Graphs stored in documents expose outputs as dependency-graph nodes with revision tagging and failure states. | T1 | 3 |
| CORE-NODES-016 | Command-batch outputs | Produces command invocations as output that the host applies as one transaction group, never mutating the document during evaluation. | T2 | 3 |
| CORE-NODES-017 | Evaluation profiling | Records per-node evaluation time and cache hits for display in the editor. | T2 | 3 |

### Serialization and assets

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-NODES-018 | Document serialization | Serializes graphs into document payloads and restores them identically. | T1 | 3 |
| CORE-NODES-019 | JSON graph files | Reads and writes `.arcgraph` files conforming to the published schema. | T1 | 3 |
| CORE-NODES-020 | Node kind migrations | Migrates stored nodes of older kind versions on load. | T1 | 3 |
| CORE-NODES-021 | Unknown node preservation | Loads unknown node kinds as placeholders keeping sockets, parameters and links, and saves them unchanged. | T1 | 3 |
| CORE-NODES-022 | Subgraphs | Collapses selections into subgraphs and expands them back, preserving links and results. | T1 | 3 |
| CORE-NODES-023 | Graph assets | Publishes graphs as versioned assets; references record identifier, version and content hash, and updating a referenced asset is an explicit command listing affected graphs. | T2 | 3 |
