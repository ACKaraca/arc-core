# arc-mesh

Polygon meshes for display, interchange and editing: indexed triangle meshes, a half-edge mesh,
attributes, subdivision surfaces, mesh booleans, simplification, repair and geometric queries.

## Owns

- Indexed triangle meshes with grouped attributes, and the half-edge polygon mesh for editing.
- Typed attribute storage on vertices, corners, faces and edges.
- Catmull–Clark subdivision with creases following the published OpenSubdiv rules.
- Mesh booleans (wrapping Manifold), simplification and meshlets (wrapping `meshopt`), and queries
  (wrapping `parry`).
- Plane sections of meshes, used by derived views and section caps.
- Repair operations with change reports.
- The documented, versioned encoding of meshes stored as document assets.

## Stays in products

Modeling tools (extrude, bevel, loop cut), sculpting, retopology, UV unwrapping, skinning and
deformers, which belong to MeshBuilder until a second product needs them; clash rules and grouping,
which belong to Arc Review.

## Key concepts and types

| Concept | Description |
|---|---|
| `TriMesh` | Positions, indices, attribute channels, face groups (each with a material slot and source sub-object key) |
| `PolyMesh` | Half-edge structure over n-gon faces, with boundary and non-manifold detection |
| `Attribute<T>` | Named channel with domain (vertex, corner, face, edge), type and interpolation rule |
| `SubdivOptions` | Scheme, boundary interpolation, face-varying interpolation, crease and corner data, level |
| `BooleanOp` | Union, difference, intersection on closed manifold meshes |
| `MeshQuery` | Ray, closest point, distance, intersection and containment over a bounding volume hierarchy |
| `MeshSection` | Closed and open polylines from a plane cut, with face provenance |
| `MeshEncoding` | Versioned binary encoding with optional quantization |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom` |
| Third-party | Manifold (through its Rust binding), `meshopt`, `parry` (f64 variant) |

## Test strategy

Property tests on half-edge invariants after every edit operation; subdivision conformance against
reference outputs generated out of process; boolean corpus (coplanar, coincident, tangent and
nested cases) with expected volumes and manifoldness; fuzzing of the mesh decoder; benchmarks for
booleans, simplification and ray queries; WebAssembly build except the Manifold-backed boolean,
which has a feature gate.

## Capabilities

### Mesh structures and attributes

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-MESH-001 | Indexed triangle meshes | Stores positions, 32-bit indices and attribute channels, with face groups mapping triangles to materials and source sub-object keys. | T1 | 0 |
| CORE-MESH-002 | Mesh encoding | Encodes meshes in a versioned binary form with optional position and normal quantization at a stated error bound; decoding validates every index against vertex counts. | T1 | 0 |
| CORE-MESH-003 | Normals and tangents | Computes smooth, flat and angle-weighted normals with a crease angle, and tangent frames for normal mapping. | T1 | 0 |
| CORE-MESH-004 | Half-edge polygon mesh | Represents n-gon meshes with half-edge connectivity, boundaries, and detection of non-manifold edges and vertices; invariants hold after every operation. | T1 | 4 |
| CORE-MESH-005 | Typed attributes | Stores attributes on vertices, corners, faces and edges with float, integer, vector and color types and declared interpolation; attributes follow topology edits. | T1 | 4 |
| CORE-MESH-006 | Mesh conversion | Converts between half-edge meshes and indexed triangle meshes, triangulating n-gons without flipped triangles where possible and keeping face provenance. | T1 | 4 |
| CORE-MESH-007 | Mesh measurements | Computes area, volume (for closed meshes), centroid, bounding boxes and genus. | T1 | 1 |

### Subdivision

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-MESH-008 | Catmull–Clark refinement | Refines polygon meshes uniformly to a given level; positions agree with the published rules within 1e-6 of the bounding box size. | T1 | 4 |
| CORE-MESH-009 | Creases and corners | Supports sharp and semi-sharp edge creases with fractional sharpness and corner vertices, decaying sharpness per level as the rules define. | T1 | 4 |
| CORE-MESH-010 | Boundary and face-varying interpolation | Supports boundary modes none, edges only, edges and corners, and face-varying interpolation modes for UV channels. | T1 | 4 |
| CORE-MESH-011 | Limit evaluation | Evaluates limit positions and normals at vertices and at face parameters for display and export. | T2 | 4 |
| CORE-MESH-012 | Adaptive refinement | Refines only around extraordinary vertices and creases to a screen-space or chord tolerance. | T3 | 5 |

### Booleans, simplification and repair

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-MESH-013 | Mesh booleans | Computes union, difference and intersection of closed manifold meshes; results are closed and manifold, and attributes and face provenance carry over. | T1 | 2 |
| CORE-MESH-014 | Boolean input validation | Rejects non-closed or non-manifold inputs with `mesh.boolean.invalid_input` listing the offending edges, instead of producing an invalid result. | T1 | 2 |
| CORE-MESH-015 | Simplification | Reduces triangle count to a target count or error bound while preserving boundaries, attribute seams and face groups. | T1 | 1 |
| CORE-MESH-016 | Level-of-detail chains | Builds a chain of simplified levels with recorded screen-space error for viewport selection. | T1 | 1 |
| CORE-MESH-017 | Vertex cache and meshlets | Reorders indices for vertex cache efficiency and partitions meshes into meshlets with bounds and cones. | T2 | 2 |
| CORE-MESH-018 | Mesh repair | Welds vertices within tolerance, removes degenerate and duplicate faces, orients faces consistently and fills holes up to a given edge count, reporting each change. | T1 | 1 |
| CORE-MESH-019 | Convex hulls and decomposition | Computes convex hulls and approximate convex decompositions for collision and clearance. | T2 | 3 |

### Queries and sections

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-MESH-020 | Ray queries | Returns the first or all hits of rays against meshes with triangle index, barycentric coordinates and distance, through a bounding volume hierarchy built once per mesh. | T1 | 1 |
| CORE-MESH-021 | Distance and closest point | Computes closest points between a point and a mesh and between two meshes, with distances in `f64`. | T1 | 1 |
| CORE-MESH-022 | Interference tests | Reports whether two meshes intersect, touch or are within a clearance distance, with contact points, for clash detection. | T1 | 3 |
| CORE-MESH-023 | Plane sections | Cuts meshes by a plane into closed loops (for closed meshes) and open polylines, each segment tagged with its source face group. | T1 | 2 |
| CORE-MESH-024 | Feature edges | Extracts boundary edges and edges whose dihedral angle exceeds a threshold, for hidden-line drawings and edge display. | T1 | 1 |
| CORE-MESH-025 | Silhouettes | Extracts silhouette edges for a view direction or eye point. | T1 | 2 |
