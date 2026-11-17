# arc-brep

Exact boundary-representation solids and surfaces behind an Arc-owned API: modeling operations,
persistent topological naming, exact sectioning, tessellation and STEP-grade exchange. The first
implementation delegates robust operations to the Open CASCADE kernel through the private FFI crate
`arc-brep-occt`.

## Owns

- `Shape` handles, topology traversal and conversion of exact geometry to `arc-geom` types.
- Modeling operation families, their results, histories and errors.
- `TopoName` construction, resolution and the geometric fallback.
- Validity checking, healing, mass properties and exact sections.
- Tessellation with named face groups and edge polylines.
- `ShapeData` serialization stored as document assets.
- The kernel shim, its build task (`cargo xtask occt`) and the `arc-worker` jobs for STEP and IGES
  import and isolated operations.
- The golden-model regression corpus runner.

## Stays in products

Features and feature trees, sketches as document objects, assemblies, sheet metal and other domain
operations built from these primitives; the decision which references are semantic and which are
B-rep names.

## Key concepts and types

| Concept | Description |
|---|---|
| `Shape` | Immutable, reference-counted handle to a solid, shell, face set, wire or compound |
| `TopoKind` | Vertex, edge, loop, face, shell, solid |
| `TopoName` | Persistent name: authored identifier, or (feature, role, sorted source names), plus a disambiguation index |
| `Signature` | Geometric signature used only for fallback resolution: surface or curve type, axis or normal, centroid, area or length, neighbor names |
| `OpResult` | New shape, naming table, history, warnings |
| `History` | Per output element: generated, modified or new; per input element: kept, modified, deleted |
| `Resolution` | Exact, by history, by geometry (flagged), or broken |
| `ShapeData` | Kernel-neutral header, exact representation bytes, naming table |
| `TessOptions` | Chord and angle tolerance, minimum and maximum edge length, normals and UVs |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-mesh` (tessellation output), `arc-brep-occt` (private) |
| Third-party | `cxx` (in `arc-brep-occt`); Open CASCADE shared libraries (dynamic, LGPL-2.1 with exception) |

## Test strategy

The golden-model regression corpus (volumes, areas, bounding boxes, topology counts, validity,
naming survival) on every change and kernel upgrade; property tests that perturb feature parameters
and assert reference survival or reported breakage; fuzzing of `ShapeData` decoding and of the STEP
import path inside `arc-worker`; address-sanitizer runs of `arc-brep-occt` nightly; benchmarks for
booleans, fillets and tessellation on reference parts.

## Capabilities

### Shapes and topology

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BREP-001 | Shape handles | Exposes solids, shells, faces, wires and compounds as immutable handles that are safe to share across threads; no kernel type appears in the public API. | T1 | 1 |
| CORE-BREP-002 | Topology traversal | Enumerates sub-elements by kind with adjacency (faces of an edge, edges of a face, loops, shells) in a deterministic order. | T1 | 1 |
| CORE-BREP-003 | Exact geometry access | Returns the exact curve of an edge and the exact surface of a face as `arc-geom` values; types without an `arc-geom` counterpart are returned as NURBS within tolerance. | T1 | 2 |
| CORE-BREP-004 | Shape serialization | Serializes shapes with their naming table to `ShapeData` and back; a round trip reproduces topology counts, names and geometry within tolerance. | T1 | 1 |
| CORE-BREP-005 | Validity checking | Checks closed shells, orientation, tolerances and self-intersection; every operation result is checked before it is returned, and invalid kernel results become errors. | T1 | 1 |

### Modeling operations

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BREP-006 | Primitives | Creates boxes, cylinders, cones, spheres, tori and wedges from parameters with authored names for each face. | T1 | 2 |
| CORE-BREP-007 | Extrude | Extrudes planar profiles (regions with holes) by distance, symmetric, to a face or through all, with optional draft angle; side faces are named from the profile edges. | T1 | 2 |
| CORE-BREP-008 | Revolve | Revolves profiles about an axis by an angle or full turn, naming faces from profile edges. | T1 | 2 |
| CORE-BREP-009 | Sweep and loft | Sweeps profiles along paths with fixed, Frenet or guided orientation and lofts through ordered sections with optional guide curves; self-intersecting results fail with `brep.sweep.self_intersection`. | T1 | 2 |
| CORE-BREP-010 | Booleans | Computes union, subtraction, intersection, split and imprint with an optional fuzzy tolerance; non-manifold results fail with the involved faces named. | T1 | 2 |
| CORE-BREP-011 | Fillets and chamfers | Rounds edges with constant or variable radius and chamfers by distances or distance and angle; when an edge cannot be filleted, the error names it and no partial result is returned. | T1 | 3 |
| CORE-BREP-012 | Shell and thicken | Hollows solids with a wall thickness and removed faces, and thickens surfaces into solids. | T1 | 3 |
| CORE-BREP-013 | Draft and offset faces | Applies draft angles to faces relative to a neutral plane and offsets selected faces, healing adjacent faces. | T2 | 3 |
| CORE-BREP-014 | Direct face editing | Moves, rotates, replaces and deletes faces of a solid without a feature history, healing neighbors, and reports when the result is not a valid solid. | T2 | 3 |
| CORE-BREP-015 | Surface operations | Fills boundaries with surfaces, extends and trims surfaces, and sews faces into shells and solids within a tolerance. | T2 | 3 |
| CORE-BREP-016 | Healing | Fixes small edges, gaps, wrong orientation and tolerance inconsistencies in imported shapes, reporting each fix. | T1 | 1 |
| CORE-BREP-017 | Cancellation and progress | Every operation accepts a cancellation token and reports progress; a cancelled operation returns `cancelled` and leaves inputs unchanged. | T1 | 2 |

### Naming

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BREP-018 | Authored names | Elements derived directly from authored inputs (profile edges, tagged faces) carry names built from the authored identifiers. | T1 | 3 |
| CORE-BREP-019 | History-derived names | Every other element receives a canonical name from the feature identifier, operation role and source element names; the same history produces the same name on every run and platform. | T1 | 3 |
| CORE-BREP-020 | Geometric fallback | When history yields several candidates, the geometric signature selects one and the resolution is reported as `resolved_by_geometry` for the caller to surface to the user. | T1 | 3 |
| CORE-BREP-021 | Broken reference reporting | A reference that cannot be resolved is reported as broken with its last name and signature; no element is chosen silently. | T1 | 3 |
| CORE-BREP-022 | Operation history | Each result reports generated, modified and deleted relations between input and output elements for callers that track references. | T1 | 2 |

### Queries, sections and tessellation

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BREP-023 | Mass properties | Computes volume, area, center of mass and inertia tensor with a stated relative accuracy. | T1 | 2 |
| CORE-BREP-024 | Distance and interference | Computes minimum distance, closest points and interference volumes between shapes. | T2 | 3 |
| CORE-BREP-025 | Exact sections | Sections shapes by planes into exact `arc-geom` curves and regions, each segment carrying the name of its source face. | T1 | 2 |
| CORE-BREP-026 | Tessellation | Produces triangle meshes within chord and angle tolerances with one face group per named face and edge polylines per named edge; shared edges produce watertight meshes. | T1 | 1 |
| CORE-BREP-027 | Exact hidden-line removal | Computes visible and hidden edges and silhouettes of shapes for a view frame as exact curves, for drawing output. | T2 | 3 |

### Exchange and isolation

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BREP-028 | STEP import | Reads STEP AP203, AP214 and AP242 files in `arc-worker`, returning shapes with assembly structure, names, colors and layers; a worker crash fails the import without affecting the caller. | T1 | 1 |
| CORE-BREP-029 | STEP export | Writes shapes and assembly structure with names and colors to STEP AP214 or AP242; written files re-import with identical topology counts. | T1 | 3 |
| CORE-BREP-030 | IGES exchange | Reads and writes IGES 5.3 surfaces and solids through the same isolation path. | T2 | 3 |
| CORE-BREP-031 | Product manufacturing information | Reads semantic and graphical dimensions, tolerances and annotations from AP242 where present and exposes them as neutral records. | T3 | 4 |
| CORE-BREP-032 | Isolated execution | Runs any modeling operation in `arc-worker` when the caller requests isolation, with identical results to in-process execution. | T2 | 3 |
| CORE-BREP-033 | Golden-model corpus | Runs the regression corpus and fails when any model's metrics leave its stated tolerances or a naming survival check fails. | T1 | 1 |
| CORE-BREP-034 | Native operation families | Allows an operation family to switch from the kernel to a native Rust implementation by configuration once it passes the corpus, without changing the public API. | T3 | 5 |
