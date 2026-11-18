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
| `TriMesh` | Positions, indices, attribute channels, face groups (each with a material slot and source sub-object key); built and validated by `TriMeshBuilder` |
| `AttributeKey`, `Channel` | The key of a vertex channel (normal, tangent, UV set, color, user name) and its `f32` values with one to four components |
| `FaceGroup`, `SubObjectKey` | A triangle range with a material slot and the `(ObjectId, u32 local key)` of its source |
| `PolyMesh` | Half-edge structure over n-gon faces, with boundary and non-manifold detection |
| `Attribute<T>` | Named channel with domain (vertex, corner, face, edge), type and interpolation rule |
| `SubdivOptions` | Scheme, boundary interpolation, face-varying interpolation, crease and corner data, level |
| `BooleanOp` | Union, difference, intersection on closed manifold meshes |
| `MeshQuery` | Ray, closest point, distance, intersection and containment over a bounding volume hierarchy |
| `MeshSection` | Closed and open polylines from a plane cut, with face provenance |
| `MeshEncoding`, `Quantization` | Versioned binary encoding with optional quantization, and its error bounds |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom` |
| Third-party | Manifold (through its Rust binding), `meshopt`, `parry` (f64 variant), each from the phase of the first capability that needs it; none in phase 0 |

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

### Triangle mesh rules

`TriMesh` (CORE-MESH-001) holds `f64` positions in model coordinates, triangles of three `u32`
vertex indices, attribute channels and face groups. It is immutable; `TriMeshBuilder::build`
validates every part against an `arc_base::Limits` profile, and `into_builder` reopens a mesh.

- **Channels** are keyed by `AttributeKey` and hold one `f32` value per vertex. The key fixes the
  component type: `Normal` three components, `Tangent` four (direction and a handedness of exactly
  1 or −1; the bitangent is handedness · (normal × tangent)), `Uv(set)` two, `Color` four (red,
  green, blue, alpha), `User(name)` one to four with a name that is not empty and at most
  `string_bytes` long. A mesh holds at most 64 channels, kept in key order. Attributes on corners,
  faces and edges belong to CORE-MESH-005.
- **Face groups** are triangle ranges `start..end` with a material slot and a `SubObjectKey`: the
  `u32` local key of the source sub-object and, when the holder of the mesh does not already name
  it, the source `ObjectId`. Groups are not empty, lie inside the triangles, are in triangle order
  and do not overlap. Triangles between groups belong to no group; `group_of` finds the group of a
  triangle.
- **Counts** of vertices and triangles must fit `u32` and `array_length`.
- **Zero.** The sign of zero is not part of a value: −0.0 is stored as +0.0 in positions and
  channels, so equal meshes have equal bits.
- **Degenerate triangles** are reported by `degenerate_triangles`, never removed: a triangle is
  degenerate when two of its indices are equal or its smallest height (twice its area over its
  longest edge) is at most the linear tolerance.
- **Merging** concatenates meshes in order, offsetting indices and group ranges. The meshes must
  have the same channel keys with the same component types.
- **Transforms** (`transformed`) map positions by the `Transform3`, normals by the inverse
  transpose of its linear part and tangent directions by the linear part, both normalized
  afterwards. Under a mirror the handedness of every tangent is negated and every triangle's
  second and third index are swapped, so the winding keeps agreeing with the normals. A transform
  with a non-finite entry, or whose linear part divided by its largest entry has a zero
  determinant, is singular; a matrix whose bottom row is not `[0, 0, 0, 1]` is not affine and
  is rejected.
- **GPU positions** (`gpu_positions`) are the positions minus an `f64` origin, narrowed to `f32`
  after the subtraction ([camera-relative precision](../architecture/rendering.md#camera-relative-precision)).
  Channels are already `f32`.
- **From 2D.** `from_triangle_mesh2` places an `arc_geom::TriangleMesh2` in the plane
  `z = elevation`; its counterclockwise triangles face +Z.

### Normals and tangents

`with_normals(weighting, crease_angle, tolerance, limits)` and `with_tangents(uv_set, tolerance)`
implement CORE-MESH-003. Lengths and angles are computed from the edge vectors of each triangle
(differences of its vertices, which are exact for nearby vertices) scaled by their largest
magnitude, so a small triangle far from the origin keeps its precision and coordinates from
1e-300 to 1e300 work.

- **Faces.** Degenerate triangles (as above, at `tolerance.linear`) contribute nothing. A face
  normal points to the side from which the winding is counterclockwise.
- **Welding.** Vertices at exactly equal positions are smoothed together, so normals stay smooth
  across vertices that were split for texture seams. Welding within a tolerance belongs to repair
  (CORE-MESH-018).
- **Crease angle.** Around a corner of triangle `t`, a face `f` at the same position counts when
  the angle between the normals of `f` and `t` is not greater than the crease angle, in radians
  within [0, π]; `t` always counts. π smooths everything. The faces around a position are kept
  in a tree over their normals, so the cost grows with the faces near the crease boundary, not
  with all faces at the position; the search takes at most 256 steps per element of
  `limits.array_length` and fails with `mesh.limit.exceeded` beyond that.
- **Weighting.** `Flat` gives each corner its own face normal and ignores the crease angle; `Area`
  sums the counted face normals weighted by face area; `Angle` weights them by the face's interior
  angle at that position. The sum is normalized.
- **Fallbacks.** A corner of a degenerate triangle takes the weighted normal of all faces at its
  position (area-weighted for `Flat`), without a crease test. A corner or vertex with no
  contribution, a sum that cancels to zero and a vertex no triangle uses get +Z.
- **Vertex order.** Vertex `i` of the input stays vertex `i` and takes the normal of its first
  corner in triangle order. Each further corner whose `f32` normal differs moves to a copy of the
  vertex appended after the input vertices in order of first use; corners with equal normals share
  a copy. Copies carry every channel. The tangent channel is removed. The same input always gives
  the same mesh.
- **Tangents** need the normal channel and the UV set. For each non-degenerate triangle whose UV
  determinant `Δu₁Δv₂ − Δu₂Δv₁` is not zero, the unit directions of increasing `u` and `v` are
  summed at its three vertices, weighted by the interior angle. The tangent is the `u` sum made
  orthogonal to the vertex normal and normalized; the handedness is −1 when normal × tangent
  points against the `v` sum and 1 otherwise. Where the `u` sum is zero or parallel to the normal (the part of it orthogonal to the normal is
  shorter than 1e-8 of its length),
  the tangent is the coordinate axis least aligned with the normal (the first of x, y, z on ties)
  made orthogonal to it, with handedness 1. A zero normal counts as +Z. No value is NaN.

Reference accuracy, checked by the tests: a cube creased below 90° has 24 vertices with exact face
normals, and at exactly 90° stays smooth; a UV sphere with `n` rings and `2n` segments has normals
within 1.6 / n² radians of radial with angle weighting and within 0.65 / n with area weighting; a
small triangle far from the origin has the normals, tangents and degeneracy of the same triangle
at the origin; and a cone apex of valence 100,000 is creased in near-linear time.

### Mesh encoding

`TriMesh::encode(quantization)` and `TriMesh::decode(bytes, limits)` write and read the byte form
`MeshEncoding::CURRENT` (`arc.mesh`, version 1) specified in
[mesh-encoding.md](../architecture/mesh-encoding.md) (CORE-MESH-002).

- Without bounds in `Quantization` the encoding is exact: decoding returns an equal mesh bit for
  bit, and the bytes of a mesh are unique.
- `position_error` (a distance) quantizes positions to a grid over the bounding box and
  `normal_angle` (radians) quantizes normals by octahedral mapping. The encoder takes the smallest
  bit width (at most 32 for positions, 24 for normals) at which every decoded value is within the
  bound, trying every width from the smallest, and stores the bound. Decoded normals have unit length. When no bit width meets a bound
  the section is stored exactly, so encoding does not fail for a tight bound; a bound that is not
  finite and positive is `mesh.quantization.invalid`.
- Decoding checks the input length, every count and every name against the limits and the
  remaining input before allocating, counts the memory it reserves against `memory_bytes`, and
  reports a reservation the platform cannot make instead of panicking; and validates the result as `TriMeshBuilder::build` does:
  every index against the vertex count and every group against the triangle count. Every failure
  is `mesh.decode.invalid` with the details `offset` and `reason`, and `cause` when validation
  rejected the mesh.

### Error codes

The codes `arc-mesh` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-mesh.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `mesh.index.out_of_range` | `invalid_input` | A triangle refers to a vertex index outside the vertex list |
| `mesh.value.not_finite` | `invalid_input` | A position or attribute value is NaN or infinite |
| `mesh.channel.length_mismatch` | `invalid_input` | An attribute channel does not hold one value per vertex |
| `mesh.channel.invalid` | `invalid_input` | An attribute channel has the wrong component type for its key, a tangent handedness other than ±1, an empty or oversized name, or the mesh has too many channels |
| `mesh.channel.missing` | `invalid_input` | An operation needs an attribute channel the mesh does not have |
| `mesh.group.invalid` | `invalid_input` | A face group is empty, out of range, out of order or overlaps another |
| `mesh.limit.exceeded` | `limit_exceeded` | A vertex, triangle or index count exceeds 32-bit indices or the limits profile |
| `mesh.transform.singular` | `invalid_input` | A transform is not finite or not invertible |
| `mesh.precision.insufficient` | `failed_operation` | A result is not representable: a transformed position overflows or a GPU coordinate exceeds `f32` |
| `mesh.merge.incompatible` | `invalid_input` | Merged meshes have different attribute channels |
| `mesh.tolerance.invalid` | `invalid_input` | A tolerance is NaN, infinite or negative |
| `mesh.crease.invalid` | `invalid_input` | A crease angle is not a finite number in [0, π] |
| `mesh.quantization.invalid` | `invalid_input` | A quantization error bound is not a finite positive number |
| `mesh.decode.invalid` | `corrupt_data` | Encoded mesh data is malformed, truncated or oversized |
| `mesh.transform.not_affine` | `invalid_input` | A transform's matrix does not have the bottom row `[0, 0, 0, 1]` of an affine transform |
